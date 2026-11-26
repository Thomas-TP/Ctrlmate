//! Cooling engine with a non-bypassable thermal safety watchdog (architecture contract section 6).
//!
//! The engine is a pure state machine: callers feed it timestamped temperature observations
//! ([`CoolingEngine::observe`]) and ask for the duties to apply ([`CoolingEngine::evaluate`]).
//! Time is always passed in, so the watchdog can be evaluated on its own timer (not only when a
//! sensor poll happens) and tests need no sleeping.
//!
//! # Rules (all paths, enforced in [`CoolingEngine::evaluate`] and nowhere else)
//!
//! 1. The pump never runs below [`MIN_PUMP_PERCENT`] (50 %): the last step of `evaluate` clamps it.
//! 2. A critical temperature on any known sensor latches the failsafe: every fan and the pump at 100 %,
//!    until every known sensor is fresh and at least `hysteresis_c` below its critical value.
//! 3. Missing data is unsafe: a sensor that was seen and is older than [`STALE_AFTER`], or *all* sensors
//!    missing for [`ALL_MISSING_AFTER`], forces fans >= [`FAILSAFE_FAN_FLOOR`] % and the pump at 100 %.
//!    A sensor lost while a temperature failsafe is latched keeps the failsafe latched.
//! 4. Write failures are reported with [`CoolingEngine::report_write`]; failed writes are retried by the
//!    actuator layer on the next tick, and [`WRITE_FAILURES_BEFORE_FAILSAFE`] failing ticks in a row
//!    escalate to the failsafe floor (reason `write_failed`).
//! 5. Defaults fit a Ryzen 9 7900X3D: CPU critical 93 C (configurable 85-98 C), GPU 88 C, liquid 55 C.

use ctrlmate_protocol::{
    CoolingPolicy, CoolingPreset, CoolingReason, CoolingState, FanCurvePoint, MIN_PUMP_PERCENT,
    PresetCurve, ThermalThresholds,
};
use std::time::{Duration, Instant};

/// A sensor that was seen and is older than this is "lost".
pub const STALE_AFTER: Duration = Duration::from_secs(5);
/// With no fresh reading from any sensor for this long the engine assumes the worst.
pub const ALL_MISSING_AFTER: Duration = Duration::from_secs(10);
/// Minimum fan duty (percent) while data is missing.
pub const FAILSAFE_FAN_FLOOR: u8 = 70;
/// Consecutive failing ticks after which a write problem escalates to the failsafe floor.
pub const WRITE_FAILURES_BEFORE_FAILSAFE: u32 = 3;
/// Hard bounds of the user-configurable thresholds.
pub const CPU_CRITICAL_RANGE: (f64, f64) = (85.0, 98.0);
pub const GPU_CRITICAL_RANGE: (f64, f64) = (70.0, 92.0);
pub const LIQUID_CRITICAL_RANGE: (f64, f64) = (40.0, 59.0);
pub const HYSTERESIS_RANGE: (f64, f64) = (2.0, 15.0);
/// Liquid temperature at which the liquid-driven fan curve starts (maps to the curve's first point).
pub const LIQUID_CURVE_START_C: f64 = 25.0;

/// Temperature sources watched by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {
    Cpu,
    Gpu,
    Liquid,
}

impl Sensor {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::Gpu, Self::Liquid];

    const fn idx(self) -> usize {
        match self {
            Self::Cpu => 0,
            Self::Gpu => 1,
            Self::Liquid => 2,
        }
    }
}

/// Which hardware channel a write belongs to (for [`CoolingEngine::report_write`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Pump,
    Fans,
}

/// Duties to apply right now. `pump_percent` is always >= [`MIN_PUMP_PERCENT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoolingTargets {
    pub effective_preset: CoolingPreset,
    pub state: CoolingState,
    pub reason: Option<CoolingReason>,
    pub pump_percent: u8,
    /// Radiator fans on the AIO.
    pub aio_fan_percent: u8,
    /// Motherboard chassis fan headers (through the deep-hardware bridge).
    pub case_fan_percent: u8,
    /// GPU fans: `None` leaves them to the firmware (the engine only takes them over in failsafe).
    pub gpu_fan_percent: Option<u8>,
}

impl CoolingTargets {
    pub fn is_failsafe(&self) -> bool {
        self.state == CoolingState::Failsafe
    }
}

#[derive(Debug, Clone, Copy)]
struct Obs {
    temp_c: f64,
    at: Instant,
}

/// Clamps user thresholds to their safe ranges (NaN falls back to the defaults).
pub fn sanitize_thresholds(t: ThermalThresholds) -> ThermalThresholds {
    let d = ThermalThresholds::default();
    let pick = |v: f64, dv: f64, r: (f64, f64)| {
        if v.is_finite() { v.clamp(r.0, r.1) } else { dv }
    };
    let liquid = pick(
        t.liquid_critical_c,
        d.liquid_critical_c,
        LIQUID_CRITICAL_RANGE,
    );
    let pump_floor_c = if t.pump_floor_c.is_finite() {
        t.pump_floor_c.clamp(LIQUID_CURVE_START_C, liquid - 5.0)
    } else {
        d.pump_floor_c.min(liquid - 5.0)
    };
    ThermalThresholds {
        cpu_critical_c: pick(t.cpu_critical_c, d.cpu_critical_c, CPU_CRITICAL_RANGE),
        gpu_critical_c: pick(t.gpu_critical_c, d.gpu_critical_c, GPU_CRITICAL_RANGE),
        liquid_critical_c: liquid,
        hysteresis_c: pick(t.hysteresis_c, d.hysteresis_c, HYSTERESIS_RANGE),
        pump_floor_c,
    }
}

/// Linear interpolation of a fan curve; flat outside the first/last point.
pub fn curve_duty(points: &[FanCurvePoint], temp_c: f64) -> u8 {
    let Some(first) = points.first() else {
        return 100;
    };
    if temp_c <= first.temp_c {
        return first.duty;
    }
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if temp_c <= b.temp_c {
            let span = b.temp_c - a.temp_c;
            if span <= 0.0 {
                return b.duty;
            }
            let f = (temp_c - a.temp_c) / span;
            let v = f64::from(a.duty) + f * (f64::from(b.duty) - f64::from(a.duty));
            return v.round().clamp(0.0, 100.0) as u8;
        }
    }
    points.last().map_or(100, |p| p.duty)
}

/// The cooling state machine.
#[derive(Debug, Clone)]
pub struct CoolingEngine {
    thresholds: ThermalThresholds,
    curves: Vec<PresetCurve>,
    requested: CoolingPreset,
    gaming: bool,
    obs: [Option<Obs>; 3],
    /// A sensor that was observed at least once (and not forgotten since).
    known: [bool; 3],
    created: Instant,
    /// Latched temperature failsafe.
    temp_latched: bool,
    write_failures: u32,
    state: CoolingState,
    reason: Option<CoolingReason>,
}

impl CoolingEngine {
    pub fn new(thresholds: ThermalThresholds, now: Instant) -> Self {
        Self {
            thresholds: sanitize_thresholds(thresholds),
            curves: CoolingPolicy::builtin().presets,
            requested: CoolingPreset::Auto,
            gaming: false,
            obs: [None; 3],
            known: [false; 3],
            created: now,
            temp_latched: false,
            write_failures: 0,
            state: CoolingState::Normal,
            reason: None,
        }
    }

    pub fn thresholds(&self) -> ThermalThresholds {
        self.thresholds
    }

    /// Runtime configuration update. Takes effect on the next `evaluate`; the values are clamped.
    pub fn update_config(&mut self, thresholds: ThermalThresholds) {
        self.thresholds = sanitize_thresholds(thresholds);
    }

    pub fn set_preset(&mut self, preset: CoolingPreset) {
        self.requested = preset;
    }

    pub fn requested_preset(&self) -> CoolingPreset {
        self.requested
    }

    /// Gaming mode reported by the agent: `auto` then behaves as `gaming`.
    pub fn set_gaming(&mut self, gaming: bool) {
        self.gaming = gaming;
    }

    pub fn effective_preset(&self) -> CoolingPreset {
        if self.requested == CoolingPreset::Auto && self.gaming {
            CoolingPreset::Gaming
        } else {
            self.requested
        }
    }

    /// Records a reading. Non-finite values are treated as "no reading".
    pub fn observe(&mut self, sensor: Sensor, temp_c: f64, now: Instant) {
        if !temp_c.is_finite() {
            return;
        }
        self.obs[sensor.idx()] = Some(Obs { temp_c, at: now });
        self.known[sensor.idx()] = true;
    }

    /// Stops watching a sensor that is intentionally gone (hardware removed, bridge stopped on
    /// purpose). A sensor that merely stops answering must NOT be forgotten: that is a failure.
    pub fn forget(&mut self, sensor: Sensor) {
        self.obs[sensor.idx()] = None;
        self.known[sensor.idx()] = false;
    }

    /// Reports the outcome of one apply tick (`ok == false`: a write failed).
    pub fn report_write(&mut self, ok: bool) {
        if ok {
            self.write_failures = 0;
        } else {
            self.write_failures = self.write_failures.saturating_add(1);
        }
    }

    fn fresh(&self, s: Sensor, now: Instant) -> Option<f64> {
        self.obs[s.idx()]
            .filter(|o| now.saturating_duration_since(o.at) <= STALE_AFTER)
            .map(|o| o.temp_c)
    }

    fn critical_of(&self, s: Sensor) -> f64 {
        match s {
            Sensor::Cpu => self.thresholds.cpu_critical_c,
            Sensor::Gpu => self.thresholds.gpu_critical_c,
            Sensor::Liquid => self.thresholds.liquid_critical_c,
        }
    }

    fn curve(&self, preset: CoolingPreset) -> PresetCurve {
        self.curves
            .iter()
            .find(|c| c.preset == preset)
            .or_else(|| self.curves.first())
            .cloned()
            .unwrap_or_else(|| CoolingPolicy::builtin().presets[0].clone())
    }

    /// Fan duty the preset asks for given the fresh readings (the hottest sensor wins).
    fn curve_fan(&self, curve: &PresetCurve, now: Instant) -> u8 {
        let mut duty = 0u8;
        let mut any = false;
        let last_t = curve.fan_curve.last().map_or(90.0, |p| p.temp_c);
        let first_t = curve.fan_curve.first().map_or(30.0, |p| p.temp_c);
        if let Some(t) = self.fresh(Sensor::Cpu, now) {
            duty = duty.max(curve_duty(&curve.fan_curve, t));
            any = true;
        }
        if let Some(t) = self.fresh(Sensor::Gpu, now) {
            duty = duty.max(curve_duty(&curve.fan_curve, t));
            any = true;
        }
        if let Some(t) = self.fresh(Sensor::Liquid, now) {
            // Liquid runs far cooler than silicon: map [25 C, critical] onto [first, last] curve temps.
            let crit = self.thresholds.liquid_critical_c;
            let f = ((t - LIQUID_CURVE_START_C) / (crit - LIQUID_CURVE_START_C)).clamp(0.0, 1.0);
            let eq = first_t + f * (last_t - first_t);
            duty = duty.max(curve_duty(&curve.fan_curve, eq));
            any = true;
        }
        if any {
            duty
        } else {
            // No data at all is handled by the failsafe; this value is only reached transiently.
            100
        }
    }

    fn pump_for(&self, curve: &PresetCurve, now: Instant) -> u8 {
        let floor = curve.pump_floor.max(MIN_PUMP_PERCENT);
        match self.fresh(Sensor::Liquid, now) {
            Some(t) if t > self.thresholds.pump_floor_c => {
                let crit = self.thresholds.liquid_critical_c;
                let span = (crit - self.thresholds.pump_floor_c).max(1.0);
                let f = ((t - self.thresholds.pump_floor_c) / span).clamp(0.0, 1.0);
                let v = f64::from(floor) + f * (100.0 - f64::from(floor));
                v.round().clamp(f64::from(floor), 100.0) as u8
            }
            _ => floor,
        }
    }

    /// Evaluates the watchdog and computes the duties to apply. Call it on a timer.
    pub fn evaluate(&mut self, now: Instant) -> CoolingTargets {
        // 1. temperature
        let mut critical_hit = false;
        for s in Sensor::ALL {
            if let Some(t) = self.fresh(s, now)
                && t >= self.critical_of(s)
            {
                critical_hit = true;
            }
        }
        if critical_hit {
            self.temp_latched = true;
        } else if self.temp_latched {
            // Leave only when every known sensor is fresh and clear of its threshold by the hysteresis.
            let h = self.thresholds.hysteresis_c;
            let all_clear = Sensor::ALL.iter().all(|&s| {
                if !self.known[s.idx()] {
                    return true;
                }
                self.fresh(s, now)
                    .is_some_and(|t| t <= self.critical_of(s) - h)
            });
            if all_clear {
                self.temp_latched = false;
            }
        }

        // 2. missing data
        let any_known_stale = Sensor::ALL
            .iter()
            .any(|&s| self.known[s.idx()] && self.fresh(s, now).is_none());
        let any_fresh = Sensor::ALL.iter().any(|&s| self.fresh(s, now).is_some());
        let newest = self
            .obs
            .iter()
            .flatten()
            .map(|o| o.at)
            .max()
            .unwrap_or(self.created);
        let all_missing = !any_fresh && now.saturating_duration_since(newest) > ALL_MISSING_AFTER;
        let sensor_lost = any_known_stale || all_missing;

        // 3. writes
        let write_failed = self.write_failures >= WRITE_FAILURES_BEFORE_FAILSAFE;

        let effective = self.effective_preset();
        let curve = self.curve(effective);
        let mut pump = self.pump_for(&curve, now);
        let mut fan = self.curve_fan(&curve, now);
        let mut gpu_fan = None;

        let (state, reason) = if self.temp_latched {
            pump = 100;
            fan = 100;
            gpu_fan = Some(100);
            (CoolingState::Failsafe, Some(CoolingReason::Temperature))
        } else if sensor_lost {
            pump = 100;
            fan = fan.max(FAILSAFE_FAN_FLOOR);
            (CoolingState::Failsafe, Some(CoolingReason::SensorLost))
        } else if write_failed {
            pump = 100;
            fan = fan.max(FAILSAFE_FAN_FLOOR);
            (CoolingState::Failsafe, Some(CoolingReason::WriteFailed))
        } else {
            (CoolingState::Normal, None)
        };
        self.state = state;
        self.reason = reason;

        // 4. the single exit: hard invariants.
        let pump = pump.clamp(MIN_PUMP_PERCENT, 100);
        let fan = fan.min(100);
        CoolingTargets {
            effective_preset: effective,
            state,
            reason,
            pump_percent: pump,
            aio_fan_percent: fan,
            case_fan_percent: fan,
            gpu_fan_percent: gpu_fan,
        }
    }

    pub fn state(&self) -> CoolingState {
        self.state
    }

    pub fn reason(&self) -> Option<CoolingReason> {
        self.reason
    }

    /// The policy the daemon publishes in its snapshot: curves, thresholds, effective state.
    pub fn policy(&self) -> CoolingPolicy {
        CoolingPolicy {
            presets: self.curves.clone(),
            thresholds: self.thresholds,
            requested_preset: self.requested,
            effective_preset: self.effective_preset(),
            state: self.state,
            reason: self.reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (CoolingEngine, Instant) {
        let t0 = Instant::now();
        (CoolingEngine::new(ThermalThresholds::default(), t0), t0)
    }

    fn feed(
        e: &mut CoolingEngine,
        now: Instant,
        cpu: Option<f64>,
        gpu: Option<f64>,
        liq: Option<f64>,
    ) {
        if let Some(v) = cpu {
            e.observe(Sensor::Cpu, v, now);
        }
        if let Some(v) = gpu {
            e.observe(Sensor::Gpu, v, now);
        }
        if let Some(v) = liq {
            e.observe(Sensor::Liquid, v, now);
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn defaults_match_the_contract() {
        let t = ThermalThresholds::default();
        assert_eq!(t.cpu_critical_c, 93.0);
        assert_eq!(t.gpu_critical_c, 88.0);
        assert_eq!(t.liquid_critical_c, 55.0);
        assert_eq!(sanitize_thresholds(t), t);
    }

    #[test]
    fn cpu_critical_is_configurable_within_85_98() {
        let mk = |v| {
            sanitize_thresholds(ThermalThresholds {
                cpu_critical_c: v,
                ..ThermalThresholds::default()
            })
            .cpu_critical_c
        };
        assert_eq!(mk(70.0), 85.0);
        assert_eq!(mk(85.0), 85.0);
        assert_eq!(mk(95.0), 95.0);
        assert_eq!(mk(120.0), 98.0);
        assert_eq!(mk(f64::NAN), 93.0);
    }

    #[test]
    fn runtime_update_config_changes_the_trip_point() {
        let (mut e, t0) = engine();
        feed(&mut e, t0, Some(90.0), Some(50.0), Some(35.0));
        assert!(
            !e.evaluate(t0).is_failsafe(),
            "90 C is fine with the 93 C default"
        );
        e.update_config(ThermalThresholds {
            cpu_critical_c: 88.0,
            ..e.thresholds()
        });
        assert_eq!(e.thresholds().cpu_critical_c, 88.0);
        let t = e.evaluate(t0);
        assert!(t.is_failsafe());
        assert_eq!(t.reason, Some(CoolingReason::Temperature));
        assert_eq!(t.aio_fan_percent, 100);
        assert_eq!(t.pump_percent, 100);
        assert_eq!(t.gpu_fan_percent, Some(100));
    }

    #[test]
    fn critical_on_any_sensor_latches_until_all_clear_by_hysteresis() {
        let (mut e, t0) = engine();
        feed(&mut e, t0, Some(60.0), Some(88.0), Some(35.0)); // GPU at critical
        let t = e.evaluate(t0);
        assert_eq!(
            (t.state, t.pump_percent, t.aio_fan_percent),
            (CoolingState::Failsafe, 100, 100)
        );
        // GPU 84 > 88 - 5: still latched
        let t1 = t0 + secs(1);
        feed(&mut e, t1, Some(60.0), Some(84.0), Some(35.0));
        assert!(e.evaluate(t1).is_failsafe());
        // GPU 83: released
        let t2 = t0 + secs(2);
        feed(&mut e, t2, Some(60.0), Some(83.0), Some(35.0));
        let t = e.evaluate(t2);
        assert_eq!(t.state, CoolingState::Normal);
        assert_eq!(t.reason, None);
    }

    #[test]
    fn liquid_and_cpu_criticals_trigger() {
        for (cpu, gpu, liq) in [(93.0, 40.0, 30.0), (40.0, 40.0, 55.0), (40.0, 88.0, 30.0)] {
            let (mut e, t0) = engine();
            feed(&mut e, t0, Some(cpu), Some(gpu), Some(liq));
            let t = e.evaluate(t0);
            assert!(t.is_failsafe(), "{cpu} {gpu} {liq}");
            assert_eq!((t.pump_percent, t.aio_fan_percent), (100, 100));
        }
    }

    #[test]
    fn stale_sensor_forces_failsafe_floor() {
        let (mut e, t0) = engine();
        feed(&mut e, t0, Some(40.0), Some(40.0), Some(30.0));
        assert!(
            !e.evaluate(t0 + secs(5)).is_failsafe(),
            "5 s old is still fresh"
        );
        // Liquid stops answering, the others keep going.
        let t = t0 + secs(7);
        feed(&mut e, t, Some(40.0), Some(40.0), None);
        let r = e.evaluate(t);
        assert_eq!(r.state, CoolingState::Failsafe);
        assert_eq!(r.reason, Some(CoolingReason::SensorLost));
        assert!(r.aio_fan_percent >= FAILSAFE_FAN_FLOOR);
        assert_eq!(r.pump_percent, 100);
        // Back: leaves.
        let t = t0 + secs(8);
        feed(&mut e, t, Some(40.0), Some(40.0), Some(30.0));
        assert_eq!(e.evaluate(t).state, CoolingState::Normal);
    }

    #[test]
    fn never_seen_sensor_is_not_a_failure() {
        // Windows zero-kernel: no CPU temperature at all. GPU + liquid carry the policy.
        let (mut e, t0) = engine();
        for i in 0..30 {
            let t = t0 + secs(i);
            feed(&mut e, t, None, Some(45.0), Some(30.0));
            assert_eq!(e.evaluate(t).state, CoolingState::Normal);
        }
    }

    #[test]
    fn all_sensors_missing_for_10s_is_unsafe() {
        let (mut e, t0) = engine();
        assert_eq!(e.evaluate(t0 + secs(9)).state, CoolingState::Normal);
        let r = e.evaluate(t0 + secs(11));
        assert_eq!(r.state, CoolingState::Failsafe);
        assert_eq!(r.reason, Some(CoolingReason::SensorLost));
        assert!(r.aio_fan_percent >= FAILSAFE_FAN_FLOOR);
        assert_eq!(r.pump_percent, 100);
    }

    #[test]
    fn sensor_lost_while_latched_stays_failsafe() {
        let (mut e, t0) = engine();
        feed(&mut e, t0, Some(95.0), Some(40.0), Some(30.0));
        assert!(e.evaluate(t0).is_failsafe());
        // The hot CPU sensor disappears; everything else is cool and fresh.
        for i in 1..40 {
            let t = t0 + secs(i);
            feed(&mut e, t, None, Some(40.0), Some(30.0));
            let r = e.evaluate(t);
            assert!(r.is_failsafe(), "left failsafe at +{i}s");
            assert_eq!(r.pump_percent, 100);
        }
        // Even once the stale sensor is gone for good, only fresh cool data releases it.
        let t = t0 + secs(41);
        feed(&mut e, t, Some(70.0), Some(40.0), Some(30.0));
        assert_eq!(e.evaluate(t).state, CoolingState::Normal);
    }

    #[test]
    fn deliberately_forgotten_sensor_does_not_trip() {
        let (mut e, t0) = engine();
        feed(&mut e, t0, Some(50.0), Some(40.0), Some(30.0));
        e.forget(Sensor::Cpu);
        let t = t0 + secs(8);
        feed(&mut e, t, None, Some(40.0), Some(30.0));
        assert_eq!(e.evaluate(t).state, CoolingState::Normal);
    }

    #[test]
    fn repeated_write_failures_escalate_and_recover() {
        let (mut e, t0) = engine();
        feed(&mut e, t0, Some(50.0), Some(40.0), Some(30.0));
        for _ in 0..WRITE_FAILURES_BEFORE_FAILSAFE - 1 {
            e.report_write(false);
        }
        assert_eq!(e.evaluate(t0).state, CoolingState::Normal);
        e.report_write(false);
        let r = e.evaluate(t0);
        assert_eq!(r.reason, Some(CoolingReason::WriteFailed));
        assert_eq!(r.pump_percent, 100);
        assert!(r.aio_fan_percent >= FAILSAFE_FAN_FLOOR);
        e.report_write(true);
        assert_eq!(e.evaluate(t0).state, CoolingState::Normal);
    }

    #[test]
    fn gaming_replaces_auto_only() {
        let (mut e, _) = engine();
        e.set_gaming(true);
        assert_eq!(e.effective_preset(), CoolingPreset::Gaming);
        e.set_preset(CoolingPreset::Silent);
        assert_eq!(e.effective_preset(), CoolingPreset::Silent);
        e.set_preset(CoolingPreset::Auto);
        e.set_gaming(false);
        assert_eq!(e.effective_preset(), CoolingPreset::Auto);
        assert_eq!(e.policy().requested_preset, CoolingPreset::Auto);
    }

    #[test]
    fn presets_follow_their_curves_and_gaming_is_louder_than_silent() {
        let at = |p: CoolingPreset| {
            let (mut e, t0) = engine();
            e.set_preset(p);
            feed(&mut e, t0, Some(60.0), Some(55.0), Some(33.0));
            e.evaluate(t0)
        };
        let silent = at(CoolingPreset::Silent);
        let gaming = at(CoolingPreset::Gaming);
        assert!(gaming.aio_fan_percent > silent.aio_fan_percent);
        assert!(gaming.pump_percent >= silent.pump_percent);
        assert_eq!(silent.pump_percent, 50);
        assert_eq!(gaming.pump_percent, 90);
    }

    #[test]
    fn pump_ramps_with_liquid_temperature() {
        let (mut e, t0) = engine();
        e.set_preset(CoolingPreset::Silent);
        feed(&mut e, t0, Some(50.0), Some(50.0), Some(45.0));
        let r = e.evaluate(t0);
        assert!(
            r.pump_percent > 50 && r.pump_percent < 100,
            "{}",
            r.pump_percent
        );
    }

    #[test]
    fn curve_interpolation() {
        let pts = [
            FanCurvePoint {
                temp_c: 30.0,
                duty: 30,
            },
            FanCurvePoint {
                temp_c: 50.0,
                duty: 50,
            },
            FanCurvePoint {
                temp_c: 70.0,
                duty: 100,
            },
        ];
        assert_eq!(curve_duty(&pts, 10.0), 30);
        assert_eq!(curve_duty(&pts, 40.0), 40);
        assert_eq!(curve_duty(&pts, 60.0), 75);
        assert_eq!(curve_duty(&pts, 200.0), 100);
        assert_eq!(curve_duty(&[], 20.0), 100);
    }

    #[test]
    fn builtin_curves_never_ask_for_less_than_a_safe_pump() {
        for c in CoolingPolicy::builtin().presets {
            assert!(c.pump_floor >= MIN_PUMP_PERCENT);
        }
    }

    #[test]
    fn non_finite_readings_are_ignored() {
        let (mut e, t0) = engine();
        e.observe(Sensor::Cpu, f64::NAN, t0);
        e.observe(Sensor::Gpu, f64::INFINITY, t0);
        // Nothing was recorded, so nothing is "known".
        assert_eq!(e.evaluate(t0 + secs(1)).state, CoolingState::Normal);
    }

    /// Tiny deterministic PRNG for the property test (no external crates).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn temp(&mut self) -> f64 {
            // -20 .. 130 C with extremes over-represented
            match self.below(8) {
                0 => 130.0,
                1 => -20.0,
                2 => 0.0,
                _ => self.below(1500) as f64 / 10.0 - 20.0,
            }
        }
    }

    #[test]
    fn property_no_path_writes_pump_below_50_and_critical_means_full() {
        let presets = CoolingPreset::all();
        for seed in 1..=300u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let t0 = Instant::now();
            let mut e = CoolingEngine::new(
                ThermalThresholds {
                    cpu_critical_c: rng.temp(),
                    gpu_critical_c: rng.temp(),
                    liquid_critical_c: rng.temp(),
                    hysteresis_c: rng.temp(),
                    pump_floor_c: rng.temp(),
                },
                t0,
            );
            let mut now = t0;
            for _ in 0..400 {
                now += Duration::from_millis(rng.below(4000));
                match rng.below(10) {
                    0 => e.set_preset(presets[rng.below(5) as usize]),
                    1 => e.set_gaming(rng.below(2) == 0),
                    2 => e.report_write(rng.below(2) == 0),
                    3 => {
                        e.update_config(ThermalThresholds {
                            cpu_critical_c: rng.temp(),
                            gpu_critical_c: rng.temp(),
                            liquid_critical_c: rng.temp(),
                            hysteresis_c: rng.temp(),
                            pump_floor_c: rng.temp(),
                        });
                    }
                    4 => e.forget(Sensor::ALL[rng.below(3) as usize]),
                    _ => {}
                }
                for s in Sensor::ALL {
                    if rng.below(3) != 0 {
                        e.observe(s, rng.temp(), now);
                    }
                }
                let r = e.evaluate(now);
                assert!(
                    r.pump_percent >= MIN_PUMP_PERCENT,
                    "seed {seed}: pump {}",
                    r.pump_percent
                );
                assert!(
                    r.pump_percent <= 100 && r.aio_fan_percent <= 100 && r.case_fan_percent <= 100
                );
                let th = e.thresholds();
                assert!(th.cpu_critical_c >= 85.0 && th.cpu_critical_c <= 98.0);
                // Any fresh reading at/over its critical value => everything at 100 %.
                let crit = [
                    (Sensor::Cpu, th.cpu_critical_c),
                    (Sensor::Gpu, th.gpu_critical_c),
                    (Sensor::Liquid, th.liquid_critical_c),
                ];
                for (s, c) in crit {
                    if e.fresh(s, now).is_some_and(|t| t >= c) {
                        assert_eq!(r.state, CoolingState::Failsafe, "seed {seed}");
                        assert_eq!(
                            (r.pump_percent, r.aio_fan_percent, r.case_fan_percent),
                            (100, 100, 100)
                        );
                        assert_eq!(r.gpu_fan_percent, Some(100));
                    }
                }
                if r.state == CoolingState::Failsafe {
                    assert!(r.reason.is_some());
                    assert!(r.aio_fan_percent >= FAILSAFE_FAN_FLOOR, "seed {seed}");
                    assert_eq!(r.pump_percent, 100);
                } else {
                    assert!(r.reason.is_none());
                }
            }
        }
    }

    #[test]
    fn policy_exposes_everything_the_snapshot_needs() {
        let (mut e, t0) = engine();
        e.set_preset(CoolingPreset::Performance);
        feed(&mut e, t0, Some(96.0), None, None);
        e.evaluate(t0);
        let p = e.policy();
        assert_eq!(p.presets.len(), 5);
        assert_eq!(p.state, CoolingState::Failsafe);
        assert_eq!(p.reason, Some(CoolingReason::Temperature));
        assert_eq!(p.requested_preset, CoolingPreset::Performance);
        assert!(p.presets.iter().all(|c| c.pump_floor >= MIN_PUMP_PERCENT));
    }
}
