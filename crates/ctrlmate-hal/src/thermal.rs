//! The cooling runtime: a dedicated thread that, on its **own timer** (1 s), samples the thermally
//! critical sensors (GPU, Kraken coolant, CPU via hwmon or the deep-hardware bridge), runs the
//! [`CoolingEngine`] watchdog and writes the result to the hardware. It does not depend on the
//! daemon's (adaptive, up to 5 s) telemetry poll: that poll only *reads* the latest [`Telemetry`].
//!
//! Hardware access goes through [`ThermalIo`], so the whole loop (sensor loss, failed writes that are
//! retried, restore on shutdown) is tested with a mock.
//!
//! Rules implemented here on top of the engine:
//! * writes are damped (raise at once, lower only by >= 3 points) so HID is not rewritten every second;
//! * a write that failed is never recorded as applied and is retried on the next tick, logged at `error`;
//! * the pump write and the fan write are independent (see `KrakenDriver::apply`);
//! * on shutdown the Kraken default curve, the GPU auto fan and the motherboard PWM (bridge `reset_pwm`,
//!   confirmed) are restored.

use crate::gpu::{FanControlUnavailable, GpuProvider, GpuSample};
use crate::kraken::{ApplyReport, KrakenDriver, KrakenStatus};
use crate::lhm_bridge::{DeepSensorSnapshot, LhmBridgeManager};
use crate::system::CpuTemps;
use anyhow::Result;
use ctrlmate_cooling::{CoolingEngine, CoolingTargets, Sensor};
use ctrlmate_protocol::{CoolingPolicy, CoolingPreset, CoolingState, ThermalThresholds};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Watchdog period.
pub const TICK: Duration = Duration::from_secs(1);
/// Telemetry older than this is not served to the poll (the thread is stuck or stopped).
pub const MAX_TELEMETRY_AGE: Duration = Duration::from_secs(5);
/// A lower duty is written only when it is at least this much below the applied one.
pub const LOWER_STEP: u8 = 3;

#[derive(Debug, Clone, Default)]
pub enum KrakenReading {
    #[default]
    Absent,
    /// The cooler exists but the read failed: no value is invented.
    Error,
    Ok(KrakenStatus),
}

/// Latest critical-sensor readings.
#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    pub sampled_at: Option<Instant>,
    pub gpus: Vec<GpuSample>,
    pub kraken: KrakenReading,
    pub cpu: CpuTemps,
    /// Fresh bridge snapshot, `None` without a (healthy) bridge.
    pub deep: Option<DeepSensorSnapshot>,
}

impl Telemetry {
    /// `None` when too old to be trusted.
    pub fn fresh(&self, now: Instant) -> Option<&Self> {
        self.sampled_at
            .filter(|t| now.saturating_duration_since(*t) <= MAX_TELEMETRY_AGE)
            .map(|_| self)
    }
}

/// Everything the loop touches in the real world.
pub trait ThermalIo: Send {
    fn sample_gpu(&mut self) -> Vec<GpuSample>;
    fn has_gpu_fan_control(&self) -> bool;
    /// `None` = no Kraken.
    fn kraken_status(&mut self) -> Option<Result<KrakenStatus>>;
    fn kraken_applied(&self) -> (Option<u8>, Option<u8>);
    fn kraken_apply(
        &mut self,
        pump: u8,
        fan: Option<u8>,
        liquid_critical_c: f64,
    ) -> Option<ApplyReport>;
    fn gpu_fan(&mut self, percent: Option<u8>) -> Result<()>;
    fn cpu_native(&mut self) -> CpuTemps;
    fn bridge_snapshot(&mut self) -> Option<DeepSensorSnapshot>;
    fn bridge_running(&self) -> bool;
    fn set_case_fans(&mut self, percent: Option<u8>) -> Result<()>;
    fn replace_kraken(&mut self, k: Option<KrakenDriver>);
    fn invalidate_kraken(&mut self);
    /// Hands everything back to firmware control. Called once at shutdown.
    fn restore(&mut self);
}

/// Production implementation.
pub struct RealThermalIo {
    pub kraken: Option<KrakenDriver>,
    pub gpu: GpuProvider,
    pub bridge: Arc<LhmBridgeManager>,
    /// The GPU fans were forced by us at some point, so they must be handed back on shutdown.
    pub gpu_overridden: bool,
}

impl ThermalIo for RealThermalIo {
    fn sample_gpu(&mut self) -> Vec<GpuSample> {
        self.gpu.poll()
    }
    fn has_gpu_fan_control(&self) -> bool {
        self.gpu.device_count() > 0
    }
    fn kraken_status(&mut self) -> Option<Result<KrakenStatus>> {
        self.kraken.as_ref().map(KrakenDriver::poll_status)
    }
    fn kraken_applied(&self) -> (Option<u8>, Option<u8>) {
        self.kraken
            .as_ref()
            .map_or((None, None), KrakenDriver::applied)
    }
    fn kraken_apply(&mut self, pump: u8, fan: Option<u8>, crit: f64) -> Option<ApplyReport> {
        self.kraken.as_mut().map(|k| k.apply(pump, fan, crit))
    }
    fn gpu_fan(&mut self, percent: Option<u8>) -> Result<()> {
        let r = self.gpu.apply_fan_target(percent);
        let unavailable = r
            .as_ref()
            .err()
            .is_some_and(|e| e.downcast_ref::<FanControlUnavailable>().is_some());
        match percent {
            // A partial failure may still have forced some fans: treat them as ours.
            Some(_) if !unavailable => self.gpu_overridden = true,
            None if r.is_ok() => self.gpu_overridden = false,
            _ => {}
        }
        r
    }
    fn cpu_native(&mut self) -> CpuTemps {
        #[cfg(target_os = "linux")]
        {
            crate::system::read_linux_cpu_temps()
        }
        #[cfg(not(target_os = "linux"))]
        {
            CpuTemps::default()
        }
    }
    fn bridge_snapshot(&mut self) -> Option<DeepSensorSnapshot> {
        self.bridge.latest()
    }
    fn bridge_running(&self) -> bool {
        self.bridge.is_running()
    }
    fn set_case_fans(&mut self, percent: Option<u8>) -> Result<()> {
        self.bridge.set_case_fans(percent).map(|_| ())
    }
    fn replace_kraken(&mut self, k: Option<KrakenDriver>) {
        // Dropping the old driver writes the default curves back.
        self.kraken = k;
    }
    fn invalidate_kraken(&mut self) {
        if let Some(k) = self.kraken.as_mut() {
            k.invalidate();
        }
    }
    fn restore(&mut self) {
        if let Some(mut k) = self.kraken.take()
            && let Err(e) = k.restore_defaults()
        {
            error!("Shutdown: {e:#}");
        }
        if self.gpu_overridden
            && let Err(e) = self.gpu.apply_fan_target(None)
        {
            error!("Shutdown: GPU fans could not be handed back to the firmware: {e:#}");
        }
        let out = self.bridge.stop();
        if out.was_running && !out.pwm_reset_verified {
            error!("Shutdown: motherboard PWM reset was not confirmed by the bridge");
        }
    }
}

/// Lowers a duty only when it is clearly below the applied one; raising is always immediate.
/// The result is never below `target`.
pub fn damped(applied: Option<u8>, target: u8) -> u8 {
    match applied {
        Some(a) if a >= target && a - target < LOWER_STEP => a,
        _ => target,
    }
}

struct Control {
    engine: CoolingEngine,
    /// What was last written to the GPU fans: `None` = nothing known/written (the firmware curve is
    /// active), `Some(None)` = handed back to the firmware, `Some(Some(p))` = forced to `p` %.
    applied_gpu: Option<Option<u8>>,
    /// NVML refused fan control (no permission / not supported): stop trying, the GPU keeps its
    /// own curve. This is not a write failure and must not push the engine into the failsafe.
    gpu_fan_unavailable: bool,
    applied_case: Option<u8>,
    seen_bridge_gen: u64,
    last_targets: Option<CoolingTargets>,
}

/// One watchdog iteration: sample, observe, evaluate, apply. Pure over `io` + `ctl`.
fn tick(
    io: &mut dyn ThermalIo,
    ctl: &mut Control,
    bridge_gen: u64,
    bridge_wanted: bool,
    now: Instant,
) -> Telemetry {
    let gpus = io.sample_gpu();
    let kraken_raw = io.kraken_status();
    let cpu = io.cpu_native();
    let deep = io.bridge_snapshot();

    // --- observe
    match gpus.iter().filter_map(|g| g.temperature_c).reduce(f64::max) {
        Some(t) => ctl.engine.observe(Sensor::Gpu, t, now),
        None if gpus.is_empty() => ctl.engine.forget(Sensor::Gpu),
        None => {} // GPU present but unreadable: it will go stale, which is unsafe by design
    }
    let kraken = match &kraken_raw {
        None => {
            ctl.engine.forget(Sensor::Liquid);
            KrakenReading::Absent
        }
        Some(Ok(st)) => {
            if let Some(l) = st.liquid_temp_c {
                ctl.engine.observe(Sensor::Liquid, l, now);
            }
            KrakenReading::Ok(st.clone())
        }
        Some(Err(e)) => {
            warn!("Kraken status read failed: {e:#}");
            KrakenReading::Error
        }
    };
    let cpu_t = cpu
        .package_c
        .or_else(|| deep.as_ref().and_then(|d| d.cpu_temp_c));
    match cpu_t {
        Some(t) => ctl.engine.observe(Sensor::Cpu, t, now),
        // Zero-kernel Windows (or an anti-cheat pause) has no CPU temperature by design; that is
        // not a sensor failure. A *wanted* bridge that stops delivering IS one (stays known, goes stale).
        None if !bridge_wanted => ctl.engine.forget(Sensor::Cpu),
        None => {}
    }

    // --- evaluate
    let t = ctl.engine.evaluate(now);
    if t.is_failsafe() && ctl.last_targets.is_none_or(|l| !l.is_failsafe()) {
        error!("Cooling failsafe engaged: {:?} ({:?})", t.reason, t);
    } else if !t.is_failsafe() && ctl.last_targets.is_some_and(|l| l.is_failsafe()) {
        info!("Cooling failsafe released");
    }

    // --- apply (every channel independently)
    let mut all_ok = true;
    let crit = ctl.engine.thresholds().liquid_critical_c;
    let (ap_pump, ap_fan) = io.kraken_applied();
    let pump = damped(ap_pump, t.pump_percent);
    let fan = damped(ap_fan, t.aio_fan_percent);
    if let Some(rep) = io.kraken_apply(pump, Some(fan), crit)
        && !rep.all_ok()
    {
        all_ok = false; // already logged at error by the driver; retried next tick
    }
    if io.has_gpu_fan_control() && !ctl.gpu_fan_unavailable {
        let wanted = t.gpu_fan_percent;
        match (ctl.applied_gpu, wanted) {
            // Never overridden and the engine wants the firmware curve: it is already active, so
            // there is nothing to write (and nothing to fail on a machine without NVML fan rights).
            (None, None) => ctl.applied_gpu = Some(None),
            (applied, wanted) if applied != Some(wanted) => match io.gpu_fan(wanted) {
                Ok(()) => ctl.applied_gpu = Some(wanted),
                Err(e) if e.downcast_ref::<FanControlUnavailable>().is_some() => {
                    warn!(
                        "{e:#}; the GPU keeps its own fan curve (run the service as administrator/root to allow fan control)"
                    );
                    ctl.gpu_fan_unavailable = true;
                }
                Err(e) => {
                    all_ok = false;
                    error!("GPU fan write failed (will retry): {e:#}");
                }
            },
            _ => {}
        }
    }
    if io.bridge_running() {
        if ctl.seen_bridge_gen != bridge_gen {
            // A (re)started bridge forgot every PWM override: write again.
            ctl.seen_bridge_gen = bridge_gen;
            ctl.applied_case = None;
        }
        let case = damped(ctl.applied_case, t.case_fan_percent);
        if ctl.applied_case != Some(case) {
            match io.set_case_fans(Some(case)) {
                Ok(()) => ctl.applied_case = Some(case),
                Err(e) => {
                    all_ok = false;
                    error!("Case fan write failed (will retry): {e:#}");
                }
            }
        }
    } else {
        ctl.applied_case = None;
    }
    ctl.engine.report_write(all_ok);
    ctl.last_targets = Some(t);

    Telemetry {
        sampled_at: Some(now),
        gpus,
        kraken,
        cpu,
        deep,
    }
}

struct RtShared {
    io: Mutex<Box<dyn ThermalIo>>,
    ctl: Mutex<Control>,
    tel: Mutex<Telemetry>,
    stop: Mutex<bool>,
    cv: Condvar,
    bridge_gen: AtomicU64,
    bridge_wanted: AtomicBool,
    restored: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Handle to the cooling thread.
pub struct CoolingRuntime {
    shared: Arc<RtShared>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl CoolingRuntime {
    /// Starts the watchdog thread. The first tick runs immediately.
    pub fn spawn(io: Box<dyn ThermalIo>, thresholds: ThermalThresholds, period: Duration) -> Self {
        let now = Instant::now();
        let shared = Arc::new(RtShared {
            io: Mutex::new(io),
            ctl: Mutex::new(Control {
                engine: CoolingEngine::new(thresholds, now),
                applied_gpu: None,
                gpu_fan_unavailable: false,
                applied_case: None,
                seen_bridge_gen: 0,
                last_targets: None,
            }),
            tel: Mutex::new(Telemetry::default()),
            stop: Mutex::new(false),
            cv: Condvar::new(),
            bridge_gen: AtomicU64::new(0),
            bridge_wanted: AtomicBool::new(false),
            restored: AtomicBool::new(false),
        });
        let s = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("ctrlmate-cooling".into())
            .spawn(move || {
                loop {
                    {
                        let mut io = lock(&s.io);
                        let mut ctl = lock(&s.ctl);
                        let tel = tick(
                            io.as_mut(),
                            &mut ctl,
                            s.bridge_gen.load(Ordering::Acquire),
                            s.bridge_wanted.load(Ordering::Acquire),
                            Instant::now(),
                        );
                        *lock(&s.tel) = tel;
                    }
                    let stop = lock(&s.stop);
                    if *stop {
                        break;
                    }
                    let (stop, _) =
                        s.cv.wait_timeout(stop, period)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if *stop {
                        break;
                    }
                }
            })
            .ok();
        Self {
            shared,
            join: Mutex::new(join),
        }
    }

    pub fn set_preset(&self, preset: CoolingPreset) {
        lock(&self.shared.ctl).engine.set_preset(preset);
    }

    pub fn set_gaming(&self, gaming: bool) {
        lock(&self.shared.ctl).engine.set_gaming(gaming);
    }

    /// Runtime configuration update (clamped by the engine); effective at the next tick.
    pub fn update_thresholds(&self, t: ThermalThresholds) {
        lock(&self.shared.ctl).engine.update_config(t);
    }

    pub fn policy(&self) -> CoolingPolicy {
        lock(&self.shared.ctl).engine.policy()
    }

    pub fn is_failsafe(&self) -> bool {
        lock(&self.shared.ctl).engine.state() == CoolingState::Failsafe
    }

    pub fn telemetry(&self) -> Telemetry {
        lock(&self.shared.tel).clone()
    }

    /// Tells the loop whether a bridge is expected (deep mode and no anti-cheat).
    pub fn set_bridge_wanted(&self, wanted: bool) {
        self.shared.bridge_wanted.store(wanted, Ordering::Release);
    }

    /// The bridge was (re)started: case-fan targets must be written again.
    pub fn bridge_started(&self) {
        self.shared.bridge_gen.fetch_add(1, Ordering::AcqRel);
        self.wake();
    }

    /// Hardware was re-probed: rewrite every target.
    pub fn invalidate(&self) {
        {
            let mut ctl = lock(&self.shared.ctl);
            ctl.applied_gpu = None;
            ctl.gpu_fan_unavailable = false;
        }
        lock(&self.shared.io).invalidate_kraken();
        self.wake();
    }

    pub fn replace_kraken(&self, k: Option<KrakenDriver>) {
        lock(&self.shared.io).replace_kraken(k);
        lock(&self.shared.ctl).last_targets = None;
        self.wake();
    }

    fn wake(&self) {
        self.shared.cv.notify_all();
    }

    /// Stops the thread and restores firmware control of everything. Idempotent.
    pub fn shutdown(&self) {
        *lock(&self.shared.stop) = true;
        self.wake();
        if let Some(j) = lock(&self.join).take() {
            let _ = j.join();
        }
        if !self.shared.restored.swap(true, Ordering::AcqRel) {
            lock(&self.shared.io).restore();
        }
    }
}

impl Drop for CoolingRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use crate::kraken::{ApplyReport, KrakenStatus};
    use std::sync::Arc;

    #[derive(Default)]
    pub struct Script {
        pub gpu_temp: Option<f64>,
        pub liquid: Option<f64>,
        pub kraken_present: bool,
        pub kraken_fail_pump: bool,
        pub kraken_fail_fan: bool,
        pub cpu: Option<f64>,
        pub bridge_running: bool,
        pub deep_cpu: Option<f64>,
        pub case_fail: bool,
        pub gpu_fan_fail: bool,
        pub gpu_fan_unavailable: bool,
        pub gpu_fan_attempts: usize,
        // recorded
        pub pump_writes: Vec<u8>,
        pub fan_writes: Vec<u8>,
        pub gpu_fan_writes: Vec<Option<u8>>,
        pub case_writes: Vec<Option<u8>>,
        pub restored: bool,
        pub applied: (Option<u8>, Option<u8>),
    }

    pub struct MockIo(pub Arc<Mutex<Script>>);

    impl ThermalIo for MockIo {
        fn sample_gpu(&mut self) -> Vec<GpuSample> {
            let s = lock(&self.0);
            s.gpu_temp
                .map(|t| {
                    vec![GpuSample {
                        temperature_c: Some(t),
                        ..GpuSample::default()
                    }]
                })
                .unwrap_or_default()
        }
        fn has_gpu_fan_control(&self) -> bool {
            lock(&self.0).gpu_temp.is_some()
        }
        fn kraken_status(&mut self) -> Option<Result<KrakenStatus>> {
            let s = lock(&self.0);
            if !s.kraken_present {
                return None;
            }
            Some(match s.liquid {
                Some(l) => Ok(KrakenStatus {
                    liquid_temp_c: Some(l),
                    ..KrakenStatus::default()
                }),
                None => Err(anyhow::anyhow!("no report")),
            })
        }
        fn kraken_applied(&self) -> (Option<u8>, Option<u8>) {
            lock(&self.0).applied
        }
        fn kraken_apply(&mut self, pump: u8, fan: Option<u8>, _c: f64) -> Option<ApplyReport> {
            let mut s = lock(&self.0);
            if !s.kraken_present {
                return None;
            }
            let mut rep = ApplyReport::default();
            if s.applied.0 != Some(pump) {
                if s.kraken_fail_pump {
                    rep.pump = Some(Err(anyhow::anyhow!("pump write failed")));
                } else {
                    s.pump_writes.push(pump);
                    s.applied.0 = Some(pump);
                    rep.pump = Some(Ok(()));
                }
            }
            if let Some(f) = fan
                && s.applied.1 != Some(f)
            {
                if s.kraken_fail_fan {
                    rep.fan = Some(Err(anyhow::anyhow!("fan write failed")));
                } else {
                    s.fan_writes.push(f);
                    s.applied.1 = Some(f);
                    rep.fan = Some(Ok(()));
                }
            }
            Some(rep)
        }
        fn gpu_fan(&mut self, percent: Option<u8>) -> Result<()> {
            let mut s = lock(&self.0);
            s.gpu_fan_attempts += 1;
            if s.gpu_fan_unavailable {
                return Err(anyhow::Error::new(FanControlUnavailable(
                    "no permission".into(),
                )));
            }
            if s.gpu_fan_fail {
                anyhow::bail!("nvml refused");
            }
            s.gpu_fan_writes.push(percent);
            Ok(())
        }
        fn cpu_native(&mut self) -> CpuTemps {
            CpuTemps {
                package_c: lock(&self.0).cpu,
                ccd_c: vec![],
            }
        }
        fn bridge_snapshot(&mut self) -> Option<DeepSensorSnapshot> {
            let s = lock(&self.0);
            (s.bridge_running).then(|| DeepSensorSnapshot {
                cpu_temp_c: s.deep_cpu,
                ..DeepSensorSnapshot::default()
            })
        }
        fn bridge_running(&self) -> bool {
            lock(&self.0).bridge_running
        }
        fn set_case_fans(&mut self, percent: Option<u8>) -> Result<()> {
            let mut s = lock(&self.0);
            if s.case_fail {
                anyhow::bail!("bridge refused");
            }
            s.case_writes.push(percent);
            Ok(())
        }
        fn replace_kraken(&mut self, _k: Option<KrakenDriver>) {}
        fn invalidate_kraken(&mut self) {
            lock(&self.0).applied = (None, None);
        }
        fn restore(&mut self) {
            lock(&self.0).restored = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;
    use ctrlmate_protocol::{CoolingReason, MIN_PUMP_PERCENT};

    fn setup(script: Script) -> (Box<dyn ThermalIo>, Control, Arc<Mutex<Script>>) {
        let s = Arc::new(Mutex::new(script));
        let ctl = Control {
            engine: CoolingEngine::new(ThermalThresholds::default(), Instant::now()),
            applied_gpu: None,
            gpu_fan_unavailable: false,
            applied_case: None,
            seen_bridge_gen: 0,
            last_targets: None,
        };
        (Box::new(MockIo(Arc::clone(&s))), ctl, s)
    }

    fn base() -> Script {
        Script {
            gpu_temp: Some(50.0),
            liquid: Some(33.0),
            kraken_present: true,
            cpu: Some(60.0),
            ..Script::default()
        }
    }

    #[test]
    fn damping_raises_at_once_and_lowers_in_steps() {
        assert_eq!(damped(None, 60), 60);
        assert_eq!(damped(Some(60), 70), 70);
        assert_eq!(damped(Some(60), 59), 60);
        assert_eq!(damped(Some(60), 58), 60);
        assert_eq!(damped(Some(60), 57), 57);
        // never below the target (so never below the 50 % pump floor)
        for a in 0..=100u8 {
            for t in 50..=100u8 {
                assert!(damped(Some(a), t) >= t);
            }
        }
    }

    #[test]
    fn normal_operation_writes_curves_and_never_a_pump_below_50() {
        let (mut io, mut ctl, s) = setup(base());
        ctl.engine.set_preset(CoolingPreset::Silent);
        let t0 = Instant::now();
        let tel = tick(io.as_mut(), &mut ctl, 0, false, t0);
        assert!(matches!(tel.kraken, KrakenReading::Ok(_)));
        let s = lock(&s);
        assert!(!s.pump_writes.is_empty() && !s.fan_writes.is_empty());
        assert!(s.pump_writes.iter().all(|&p| p >= MIN_PUMP_PERCENT));
        assert!(
            s.gpu_fan_writes.is_empty(),
            "GPU fans stay with the firmware outside failsafe: nothing is written"
        );
    }

    #[test]
    fn critical_gpu_temperature_forces_everything_to_100() {
        let (mut io, mut ctl, s) = setup(base());
        lock(&s).gpu_temp = Some(90.0);
        lock(&s).bridge_running = true;
        tick(io.as_mut(), &mut ctl, 1, true, Instant::now());
        let s = lock(&s);
        assert_eq!(s.pump_writes.last(), Some(&100));
        assert_eq!(s.fan_writes.last(), Some(&100));
        assert_eq!(s.gpu_fan_writes.last(), Some(&Some(100)));
        assert_eq!(s.case_writes.last(), Some(&Some(100)));
    }

    #[test]
    fn failed_pump_write_is_retried_next_tick_and_fan_is_written_anyway() {
        let (mut io, mut ctl, s) = setup(base());
        lock(&s).kraken_fail_pump = true;
        let t0 = Instant::now();
        tick(io.as_mut(), &mut ctl, 0, false, t0);
        {
            let g = lock(&s);
            assert!(
                g.pump_writes.is_empty(),
                "failed write must not be recorded"
            );
            assert!(!g.fan_writes.is_empty(), "fan write must not be skipped");
            assert_eq!(g.applied.0, None);
        }
        lock(&s).kraken_fail_pump = false;
        tick(io.as_mut(), &mut ctl, 0, false, t0 + Duration::from_secs(1));
        assert!(!lock(&s).pump_writes.is_empty(), "retried");
    }

    #[test]
    fn persistent_write_failures_escalate_to_the_failsafe_floor() {
        let (mut io, mut ctl, s) = setup(base());
        lock(&s).kraken_fail_pump = true;
        let t0 = Instant::now();
        for i in 0..4 {
            tick(io.as_mut(), &mut ctl, 0, false, t0 + Duration::from_secs(i));
        }
        assert_eq!(ctl.engine.policy().reason, Some(CoolingReason::WriteFailed));
        assert_eq!(ctl.engine.policy().state, CoolingState::Failsafe);
    }

    #[test]
    fn lost_kraken_status_goes_stale_and_trips_sensor_lost() {
        let (mut io, mut ctl, s) = setup(base());
        let t0 = Instant::now();
        tick(io.as_mut(), &mut ctl, 0, false, t0);
        assert_eq!(ctl.engine.policy().state, CoolingState::Normal);
        lock(&s).liquid = None; // HID reads now fail
        for i in 1..=7 {
            tick(io.as_mut(), &mut ctl, 0, false, t0 + Duration::from_secs(i));
        }
        let p = ctl.engine.policy();
        assert_eq!(p.state, CoolingState::Failsafe);
        assert_eq!(p.reason, Some(CoolingReason::SensorLost));
        let g = lock(&s);
        assert_eq!(g.pump_writes.last(), Some(&100));
        assert!(*g.fan_writes.last().unwrap() >= 70);
    }

    #[test]
    fn windows_zero_kernel_has_no_cpu_temperature_and_policy_relies_on_liquid_and_gpu() {
        let mut sc = base();
        sc.cpu = None; // no hwmon, no bridge
        let (mut io, mut ctl, s) = setup(sc);
        let t0 = Instant::now();
        for i in 0..20 {
            let tel = tick(io.as_mut(), &mut ctl, 0, false, t0 + Duration::from_secs(i));
            assert!(tel.cpu.package_c.is_none() && tel.deep.is_none());
            assert_eq!(ctl.engine.policy().state, CoolingState::Normal);
        }
        // Hot liquid alone still protects.
        lock(&s).liquid = Some(56.0);
        tick(
            io.as_mut(),
            &mut ctl,
            0,
            false,
            t0 + Duration::from_secs(21),
        );
        assert_eq!(ctl.engine.policy().reason, Some(CoolingReason::Temperature));
    }

    #[test]
    fn a_wanted_bridge_that_stops_delivering_is_a_sensor_loss_but_an_unwanted_one_is_not() {
        let mut sc = base();
        sc.cpu = None;
        sc.bridge_running = true;
        sc.deep_cpu = Some(55.0);
        let (mut io, mut ctl, s) = setup(sc);
        let t0 = Instant::now();
        tick(io.as_mut(), &mut ctl, 1, true, t0);
        // The bridge dies: its snapshot disappears.
        lock(&s).bridge_running = false;
        for i in 1..=7 {
            tick(io.as_mut(), &mut ctl, 1, true, t0 + Duration::from_secs(i));
        }
        assert_eq!(ctl.engine.policy().reason, Some(CoolingReason::SensorLost));
        // The bridge is intentionally stopped (anti-cheat): the CPU sensor is forgotten, failsafe leaves.
        tick(io.as_mut(), &mut ctl, 1, false, t0 + Duration::from_secs(9));
        assert_eq!(ctl.engine.policy().state, CoolingState::Normal);
    }

    #[test]
    fn case_fans_are_rewritten_after_a_bridge_restart_and_failures_retry() {
        let mut sc = base();
        sc.bridge_running = true;
        sc.deep_cpu = Some(50.0);
        let (mut io, mut ctl, s) = setup(sc);
        let t0 = Instant::now();
        lock(&s).case_fail = true;
        tick(io.as_mut(), &mut ctl, 1, true, t0);
        assert!(lock(&s).case_writes.is_empty());
        lock(&s).case_fail = false;
        tick(io.as_mut(), &mut ctl, 1, true, t0 + Duration::from_secs(1));
        let n = lock(&s).case_writes.len();
        assert_eq!(n, 1, "retried and applied");
        tick(io.as_mut(), &mut ctl, 1, true, t0 + Duration::from_secs(2));
        assert_eq!(
            lock(&s).case_writes.len(),
            n,
            "no rewrite while nothing changed"
        );
        // Bridge restarted: generation bumps, target is written again.
        tick(io.as_mut(), &mut ctl, 2, true, t0 + Duration::from_secs(3));
        assert_eq!(lock(&s).case_writes.len(), n + 1);
    }

    #[test]
    fn gpu_fan_failures_are_retried() {
        let (mut io, mut ctl, s) = setup(base());
        // A critical GPU temperature makes the engine force the GPU fans to 100 %.
        lock(&s).gpu_temp = Some(90.0);
        lock(&s).gpu_fan_fail = true;
        let t0 = Instant::now();
        tick(io.as_mut(), &mut ctl, 0, false, t0);
        assert!(ctl.applied_gpu.is_none());
        lock(&s).gpu_fan_fail = false;
        tick(io.as_mut(), &mut ctl, 0, false, t0 + Duration::from_secs(1));
        assert_eq!(ctl.applied_gpu, Some(Some(100)));
    }

    #[test]
    fn firmware_controlled_gpu_fans_are_never_written_and_never_fail_the_engine() {
        // Regression: a non-admin run (NVML refuses fan writes) used to latch the failsafe with
        // `WriteFailed`, because the first tick "handed the fans back" although they were never taken.
        let (mut io, mut ctl, s) = setup(base());
        lock(&s).gpu_fan_fail = true;
        lock(&s).gpu_fan_unavailable = true;
        let t0 = Instant::now();
        for i in 0..6 {
            tick(io.as_mut(), &mut ctl, 0, false, t0 + Duration::from_secs(i));
        }
        assert_eq!(
            lock(&s).gpu_fan_attempts,
            0,
            "no write while the firmware is in control"
        );
        assert_eq!(ctl.applied_gpu, Some(None));
        assert_eq!(ctl.engine.policy().state, CoolingState::Normal);
    }

    #[test]
    fn unavailable_gpu_fan_control_is_not_a_write_failure_and_is_tried_once() {
        let (mut io, mut ctl, s) = setup(base());
        lock(&s).gpu_temp = Some(90.0); // critical: the engine wants the GPU fans at 100 %
        lock(&s).gpu_fan_unavailable = true;
        let t0 = Instant::now();
        for i in 0..6 {
            tick(io.as_mut(), &mut ctl, 0, false, t0 + Duration::from_secs(i));
        }
        assert!(ctl.gpu_fan_unavailable);
        assert_eq!(lock(&s).gpu_fan_attempts, 1, "no endless retries");
        // The failsafe is there because of the temperature, not because of a failed write.
        assert_eq!(ctl.engine.policy().reason, Some(CoolingReason::Temperature));
        // The other actuators were still driven to the failsafe floor.
        assert_eq!(lock(&s).pump_writes.last(), Some(&100));
    }

    #[test]
    fn the_watchdog_runs_on_its_own_timer_and_shutdown_restores_everything() {
        let s = Arc::new(Mutex::new(base()));
        let rt = CoolingRuntime::spawn(
            Box::new(MockIo(Arc::clone(&s))),
            ThermalThresholds::default(),
            Duration::from_millis(20),
        );
        // No poll anywhere: the thread alone must observe a critical GPU and react.
        lock(&s).gpu_temp = Some(95.0);
        let t0 = Instant::now();
        while !rt.is_failsafe() && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(rt.is_failsafe());
        assert!(rt.telemetry().fresh(Instant::now()).is_some());
        assert_eq!(rt.policy().reason, Some(CoolingReason::Temperature));
        // Runtime config update path really changes behaviour.
        lock(&s).gpu_temp = Some(50.0);
        rt.update_thresholds(ThermalThresholds {
            cpu_critical_c: 90.0,
            ..ThermalThresholds::default()
        });
        assert_eq!(rt.policy().thresholds.cpu_critical_c, 90.0);
        rt.set_preset(CoolingPreset::Gaming);
        assert_eq!(rt.policy().requested_preset, CoolingPreset::Gaming);
        rt.shutdown();
        assert!(lock(&s).restored, "firmware control must be restored");
        rt.shutdown(); // idempotent
    }
}
