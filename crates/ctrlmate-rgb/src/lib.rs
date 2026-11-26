//! Ctrlmate lighting: zone discovery, hardware backends and the colour engine.
//!
//! * [`LightingEngine`] owns the *logical* state (master + per-zone on/intensity/colour) and renders
//!   smooth transitions; it is hardware agnostic and produces `(zone_id, RgbColor)` frames.
//! * [`backends::RgbHardwareDispatcher`] discovers the **real** hardware, exposes it as
//!   [`ZoneInfo`] zones (see [`zones`] for the zone-id contract) and drives every backend on its
//!   own worker thread (see [`pipeline`]) so a slow bus never blocks the caller.
//!
//! # Colour pipeline
//!
//! ```text
//! zone colour (RGB or Kelvin -> RGB)      kelvin_to_rgb: Tanner Helland curve, 6500 K == (255,255,255)
//!   x zone intensity (0..1) x master intensity (0..1) x on/off     multiplicative composition
//!   -> "perceptual" linear-in-perception triple   (transitions interpolate here, from the in-flight value)
//!   -> gamma (default 2.2: duty = value^gamma)     -> 8-bit hardware RGB
//! ```
//!
//! The gamma stage is real: Home Assistant brightness is a perceptual scale, LED duty cycle is linear
//! light, so 50 % white is sent as `0.5^2.2 * 255 = 55`. Set the gamma to `1.0` for no correction.
//! The engine has no effect "modes": a zone is always a solid colour, optionally fading.

pub mod backends;
pub mod pipeline;
pub mod zones;

pub use zones::{ZONE_GLOBAL, ZoneCapabilities, ZoneInfo};

use ctrlmate_protocol::{
    LightColorMode, LightState, MAX_COLOR_TEMP_KELVIN, MIN_COLOR_TEMP_KELVIN, RgbColor,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Default perceptual gamma applied to the final colour.
pub const DEFAULT_GAMMA: f32 = 2.2;
/// Default fade duration when the caller does not specify one.
pub const DEFAULT_TRANSITION_MS: u32 = 250;

/// Raw Tanner Helland black-body approximation (valid 1000..=40000 K), channels in 0..=255.
fn helland(kelvin: f64) -> [f64; 3] {
    let temp = kelvin / 100.0;
    let red = if temp <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (temp - 60.0).powf(-0.133_204_759_2)
    };
    let green = if temp <= 66.0 {
        99.470_802_586_1 * temp.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (temp - 60.0).powf(-0.075_514_849_2)
    };
    let blue = if temp >= 66.0 {
        255.0
    } else if temp <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (temp - 10.0).ln() - 305.044_792_730_7
    };
    [
        red.clamp(0.0, 255.0),
        green.clamp(0.0, 255.0),
        blue.clamp(0.0, 255.0),
    ]
}

/// Converts a colour temperature (`2000K..=6500K`) to RGB.
///
/// Uses the Helland black-body curve and white-balances it so that the top of the range (6500 K,
/// D65 daylight) is exactly white `(255, 255, 255)`; no blue/green bias is applied to warm values.
pub fn kelvin_to_rgb(kelvin: u16) -> RgbColor {
    let k = f64::from(kelvin.clamp(MIN_COLOR_TEMP_KELVIN, MAX_COLOR_TEMP_KELVIN));
    let raw = helland(k);
    let white = helland(f64::from(MAX_COLOR_TEMP_KELVIN));
    let ch = |i: usize| -> u8 { (raw[i] * 255.0 / white[i]).round().clamp(0.0, 255.0) as u8 };
    RgbColor {
        r: ch(0),
        g: ch(1),
        b: ch(2),
    }
}

/// Base colour of a zone state (before intensity).
fn base_color(state: &LightState) -> RgbColor {
    match state.color_mode {
        LightColorMode::Rgb => state.rgb,
        LightColorMode::ColorTemp => kelvin_to_rgb(state.color_temp_kelvin),
    }
}

/// Composes zone colour x zone intensity x master intensity into the perceptual triple (0..=1).
pub fn compose_perceptual(zone: &LightState, master: Option<&LightState>) -> [f32; 3] {
    let master_on = master.is_none_or(|m| m.is_on);
    if !zone.is_on || !master_on {
        return [0.0; 3];
    }
    let master_i = master.map_or(1.0, |m| f32::from(m.brightness) / 255.0);
    let intensity = f32::from(zone.brightness) / 255.0 * master_i;
    let base = base_color(zone);
    [
        f32::from(base.r) / 255.0 * intensity,
        f32::from(base.g) / 255.0 * intensity,
        f32::from(base.b) / 255.0 * intensity,
    ]
}

/// Applies the gamma curve and quantises to 8-bit hardware RGB.
pub fn encode_hardware(v: [f32; 3], gamma: f32) -> RgbColor {
    let g = gamma.clamp(0.5, 4.0);
    let enc = |x: f32| -> u8 {
        (x.clamp(0.0, 1.0).powf(g) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    RgbColor {
        r: enc(v[0]),
        g: enc(v[1]),
        b: enc(v[2]),
    }
}

/// Renders a single zone state (no master) to hardware RGB; `apply_gamma` selects gamma 2.2 or linear.
pub fn render_hardware_rgb(state: &LightState, apply_gamma: bool) -> RgbColor {
    encode_hardware(
        compose_perceptual(state, None),
        if apply_gamma { DEFAULT_GAMMA } else { 1.0 },
    )
}

#[derive(Debug, Clone)]
struct Transition {
    from: [f32; 3],
    to: [f32; 3],
    started_at: Instant,
    duration: Duration,
}

impl Transition {
    fn sample(&self, now: Instant) -> ([f32; 3], bool) {
        if self.duration.is_zero() {
            return (self.to, true);
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        let t = (elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t); // smoothstep
        let mix = |a: f32, b: f32| a + (b - a) * e;
        (
            [
                mix(self.from[0], self.to[0]),
                mix(self.from[1], self.to[1]),
                mix(self.from[2], self.to[2]),
            ],
            t >= 1.0,
        )
    }
}

#[derive(Debug)]
struct ZoneRuntime {
    caps: ZoneCapabilities,
    /// Last sampled (in-flight) perceptual colour.
    shown: [f32; 3],
    transition: Option<Transition>,
    last_emitted: Option<RgbColor>,
}

/// Logical lighting state: one virtual master (`light_global`) and the registered hardware zones.
#[derive(Debug)]
pub struct LightingEngine {
    states: BTreeMap<String, LightState>,
    runtime: BTreeMap<String, ZoneRuntime>,
    gamma: f32,
}

impl Default for LightingEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Applies the optional fields of an update to `state`, honouring `caps`.
fn apply_fields(
    state: &mut LightState,
    caps: ZoneCapabilities,
    is_on: Option<bool>,
    brightness: Option<u8>,
    rgb: Option<RgbColor>,
    kelvin: Option<u16>,
) {
    if let Some(on) = is_on {
        state.is_on = on;
    }
    if let (Some(b), true) = (brightness, caps.intensity) {
        state.brightness = b;
        if b > 0 && is_on.is_none() {
            state.is_on = true;
        }
    }
    apply_color_fields(state, caps, rgb, kelvin);
}

fn apply_color_fields(
    state: &mut LightState,
    caps: ZoneCapabilities,
    rgb: Option<RgbColor>,
    kelvin: Option<u16>,
) {
    if let (Some(k), true) = (kelvin, caps.kelvin) {
        let k = k.clamp(MIN_COLOR_TEMP_KELVIN, MAX_COLOR_TEMP_KELVIN);
        state.color_temp_kelvin = k;
        state.color_mode = LightColorMode::ColorTemp;
        state.rgb = kelvin_to_rgb(k);
    } else if let (Some(c), true) = (rgb, caps.rgb) {
        state.rgb = c;
        state.color_mode = LightColorMode::Rgb;
    }
}

impl LightingEngine {
    pub fn new() -> Self {
        let mut states = BTreeMap::new();
        // The master is a multiplier: full intensity by default so it is neutral until used.
        states.insert(
            ZONE_GLOBAL.to_string(),
            LightState {
                brightness: 255,
                ..LightState::default()
            },
        );
        Self {
            states,
            runtime: BTreeMap::new(),
            gamma: DEFAULT_GAMMA,
        }
    }

    pub fn gamma(&self) -> f32 {
        self.gamma
    }

    /// Sets the output gamma (clamped to 0.5..=4.0); `1.0` disables the correction.
    pub fn set_gamma(&mut self, gamma: f32) {
        self.gamma = gamma.clamp(0.5, 4.0);
    }

    /// Registers a zone with full capabilities. Callers must only register zones that a backend
    /// really drives (see [`Self::register_zones`]).
    pub fn register_zone(&mut self, entity_id: impl Into<String>, initial: LightState) {
        self.register_zone_with_caps(entity_id, initial, ZoneCapabilities::FULL);
    }

    pub fn register_zone_with_caps(
        &mut self,
        entity_id: impl Into<String>,
        initial: LightState,
        caps: ZoneCapabilities,
    ) {
        let id = entity_id.into();
        if id == ZONE_GLOBAL || self.runtime.contains_key(&id) {
            return;
        }
        let shown = compose_perceptual(&initial, self.states.get(ZONE_GLOBAL));
        self.states.insert(id.clone(), initial);
        self.runtime.insert(
            id,
            ZoneRuntime {
                caps,
                shown,
                transition: None,
                last_emitted: None,
            },
        );
    }

    /// Registers every *controllable* discovered zone; returns the ids that were skipped
    /// (not controllable). Informational zones never enter the engine.
    pub fn register_zones(&mut self, zones: &[ZoneInfo]) -> Vec<String> {
        let mut skipped = Vec::new();
        for z in zones {
            if z.controllable {
                self.register_zone_with_caps(z.id.clone(), LightState::default(), z.capabilities);
            } else {
                skipped.push(z.id.clone());
            }
        }
        skipped
    }

    /// Drops every hardware zone (keeps the master); used before re-registering after a rescan.
    pub fn clear_zones(&mut self) {
        self.runtime.clear();
        self.states.retain(|id, _| id == ZONE_GLOBAL);
    }

    pub fn zone_state(&self, entity_id: &str) -> Option<&LightState> {
        self.states.get(entity_id)
    }

    /// Master plus all zones, keyed by entity id.
    pub fn all_zones(&self) -> &BTreeMap<String, LightState> {
        &self.states
    }

    pub fn zone_capabilities(&self, entity_id: &str) -> Option<ZoneCapabilities> {
        if entity_id == ZONE_GLOBAL {
            return Some(ZoneCapabilities::FULL);
        }
        self.runtime.get(entity_id).map(|r| r.caps)
    }

    fn target_for(&self, id: &str) -> [f32; 3] {
        match self.states.get(id) {
            Some(zone) => compose_perceptual(zone, self.states.get(ZONE_GLOBAL)),
            None => [0.0; 3],
        }
    }

    fn sampled(&self, id: &str, now: Instant) -> [f32; 3] {
        match self.runtime.get(id) {
            Some(rt) => rt.transition.as_ref().map_or(rt.shown, |t| t.sample(now).0),
            None => [0.0; 3],
        }
    }

    /// Applies a partial update to a zone, or to the master when `entity_id == "light_global"`.
    ///
    /// * Master `is_on` / `brightness` are *multiplicative* with each zone and do not touch zone states.
    /// * Master `rgb` / `color_temp_kelvin` are pushed into every zone (zone-specific colours remain
    ///   possible afterwards).
    /// * Transitions restart from the **currently shown** (in-flight) colour.
    ///
    /// Returns the new `(entity_id, state)` pairs that changed (master + all zones for a master update).
    pub fn apply_update(
        &mut self,
        entity_id: &str,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    ) -> Vec<(String, LightState)> {
        self.apply_update_at(
            Instant::now(),
            entity_id,
            is_on,
            brightness,
            rgb,
            color_temp_kelvin,
            transition_ms,
        )
    }

    /// Same as [`Self::apply_update`] with an explicit clock (deterministic tests).
    #[allow(clippy::too_many_arguments)]
    pub fn apply_update_at(
        &mut self,
        now: Instant,
        entity_id: &str,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    ) -> Vec<(String, LightState)> {
        let global = entity_id == ZONE_GLOBAL;
        if !global && !self.runtime.contains_key(entity_id) {
            return Vec::new();
        }
        let duration =
            Duration::from_millis(u64::from(transition_ms.unwrap_or(DEFAULT_TRANSITION_MS)));

        // Freeze the in-flight colours before touching any state.
        let from: BTreeMap<String, [f32; 3]> = self
            .runtime
            .keys()
            .map(|id| (id.clone(), self.sampled(id, now)))
            .collect();

        let zone_ids: Vec<String> = if global {
            self.runtime.keys().cloned().collect()
        } else {
            vec![entity_id.to_string()]
        };

        if global {
            if let Some(master) = self.states.get_mut(ZONE_GLOBAL) {
                apply_fields(
                    master,
                    ZoneCapabilities::FULL,
                    is_on,
                    brightness,
                    rgb,
                    color_temp_kelvin,
                );
            }
            if rgb.is_some() || color_temp_kelvin.is_some() {
                for id in &zone_ids {
                    let caps = self
                        .runtime
                        .get(id)
                        .map_or(ZoneCapabilities::NONE, |r| r.caps);
                    if let Some(st) = self.states.get_mut(id) {
                        apply_color_fields(st, caps, rgb, color_temp_kelvin);
                    }
                }
            }
        } else {
            let caps = self
                .runtime
                .get(entity_id)
                .map_or(ZoneCapabilities::NONE, |r| r.caps);
            if let Some(st) = self.states.get_mut(entity_id) {
                apply_fields(st, caps, is_on, brightness, rgb, color_temp_kelvin);
            }
        }

        for id in &zone_ids {
            let to = self.target_for(id);
            let start = from.get(id).copied().unwrap_or([0.0; 3]);
            if let Some(rt) = self.runtime.get_mut(id) {
                rt.shown = start;
                let never_emitted = rt.last_emitted.is_none();
                rt.transition = if start == to && !never_emitted {
                    rt.shown = to;
                    None
                } else {
                    Some(Transition {
                        from: start,
                        to,
                        started_at: now,
                        duration,
                    })
                };
            }
        }

        let mut out = Vec::new();
        if global && let Some(m) = self.states.get(ZONE_GLOBAL) {
            out.push((ZONE_GLOBAL.to_string(), m.clone()));
        }
        for id in zone_ids {
            if let Some(st) = self.states.get(&id) {
                out.push((id, st.clone()));
            }
        }
        out
    }

    /// Advances active transitions and returns the frames to push (only zones whose output changed).
    pub fn tick_transitions(&mut self, now: Instant) -> Vec<(String, RgbColor)> {
        let gamma = self.gamma;
        let mut frames = Vec::new();
        for (id, rt) in &mut self.runtime {
            let Some(tr) = rt.transition.as_ref() else {
                continue;
            };
            let (v, done) = tr.sample(now);
            rt.shown = v;
            if done {
                rt.transition = None;
            }
            let hw = encode_hardware(v, gamma);
            if rt.last_emitted != Some(hw) {
                rt.last_emitted = Some(hw);
                frames.push((id.clone(), hw));
            }
        }
        frames
    }

    /// Current hardware colour of every zone (for re-synchronising a freshly discovered backend).
    pub fn current_frames(&self, now: Instant) -> Vec<(String, RgbColor)> {
        self.runtime
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    encode_hardware(self.sampled(id, now), self.gamma),
                )
            })
            .collect()
    }

    /// Forgets what was last sent so the next transition always reaches the hardware.
    pub fn invalidate_output(&mut self) {
        for rt in self.runtime.values_mut() {
            rt.last_emitted = None;
        }
    }

    /// `true` while any transition is still running.
    pub fn is_animating(&self) -> bool {
        self.runtime.values().any(|r| r.transition.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white() -> RgbColor {
        RgbColor {
            r: 255,
            g: 255,
            b: 255,
        }
    }

    #[test]
    fn kelvin_6500_is_white_and_warm_is_warm() {
        assert_eq!(kelvin_to_rgb(6500), white());
        let warm = kelvin_to_rgb(2700);
        assert_eq!(warm.r, 255);
        assert!(warm.r > warm.g && warm.g > warm.b);
        // monotone: more Kelvin -> more blue, never less
        let mut prev = 0u8;
        for k in (2000..=6500).step_by(100) {
            let b = kelvin_to_rgb(k).b;
            assert!(b >= prev, "blue decreased at {k}K");
            prev = b;
        }
        // no warm-white blue hack: 4000 K blue matches the raw curve (white-balanced), not 0.9x of it
        let k4 = kelvin_to_rgb(4000);
        assert!(k4.b > 130, "{k4:?}");
    }

    #[test]
    fn gamma_is_applied_perceptually() {
        let st = LightState {
            is_on: true,
            brightness: 128,
            color_mode: LightColorMode::Rgb,
            rgb: white(),
            color_temp_kelvin: 4000,
        };
        let lin = render_hardware_rgb(&st, false);
        let gam = render_hardware_rgb(&st, true);
        assert_eq!(lin.r, 128);
        assert_eq!(gam.r, 56); // (128/255)^2.2 * 255
        assert!(gam.r < lin.r);
    }

    #[test]
    fn off_state_renders_black() {
        let state = LightState {
            is_on: false,
            ..LightState::default()
        };
        assert_eq!(
            render_hardware_rgb(&state, true),
            RgbColor { r: 0, g: 0, b: 0 }
        );
    }

    fn engine_with(zone: &str) -> LightingEngine {
        let mut e = LightingEngine::new();
        e.set_gamma(1.0);
        e.register_zone(zone, LightState::default());
        e
    }

    #[test]
    fn master_and_zone_intensity_multiply() {
        let mut e = engine_with("light_ram");
        let t0 = Instant::now();
        e.apply_update_at(
            t0,
            "light_ram",
            Some(true),
            Some(128),
            Some(white()),
            None,
            Some(0),
        );
        e.apply_update_at(t0, "light_global", None, Some(128), None, None, Some(0));
        let f = e.tick_transitions(t0);
        assert_eq!(f.len(), 1);
        // 128/255 * 128/255 = 0.252 -> 64
        assert_eq!(f[0].1.r, 64);
        // master off silences everything but keeps zone state
        e.apply_update_at(t0, "light_global", Some(false), None, None, None, Some(0));
        let f = e.tick_transitions(t0);
        assert_eq!(f[0].1, RgbColor { r: 0, g: 0, b: 0 });
        assert_eq!(e.zone_state("light_ram").map(|s| s.brightness), Some(128));
    }

    #[test]
    fn master_colour_pushes_to_zones_but_zone_colour_is_independent() {
        let mut e = engine_with("light_ram");
        e.register_zone("light_fans", LightState::default());
        let t0 = Instant::now();
        let updates = e.apply_update_at(t0, "light_global", None, None, None, Some(2700), Some(0));
        assert_eq!(updates.len(), 3);
        assert!(updates.iter().all(|(_, s)| s.color_temp_kelvin == 2700));
        e.apply_update_at(
            t0,
            "light_fans",
            None,
            None,
            Some(RgbColor { r: 0, g: 0, b: 255 }),
            None,
            Some(0),
        );
        assert_eq!(
            e.zone_state("light_ram").map(|s| s.color_temp_kelvin),
            Some(2700)
        );
        assert_eq!(
            e.zone_state("light_fans").map(|s| s.color_mode),
            Some(LightColorMode::Rgb)
        );
    }

    #[test]
    fn transition_restarts_from_in_flight_colour() {
        let mut e = engine_with("light_ram");
        let t0 = Instant::now();
        e.apply_update_at(
            t0,
            "light_ram",
            Some(true),
            Some(255),
            Some(rgb_state_color(255, 0, 0)),
            None,
            Some(0),
        );
        e.tick_transitions(t0);
        // fade to blue over 1 s, interrupt at 500 ms with a fade to green
        e.apply_update_at(
            t0,
            "light_ram",
            None,
            None,
            Some(rgb_state_color(0, 0, 255)),
            None,
            Some(1000),
        );
        let mid = t0 + Duration::from_millis(500);
        let f = e.tick_transitions(mid);
        let in_flight = f[0].1;
        assert!(in_flight.r > 100 && in_flight.b > 100, "{in_flight:?}");
        e.apply_update_at(
            mid,
            "light_ram",
            None,
            None,
            Some(rgb_state_color(0, 255, 0)),
            None,
            Some(1000),
        );
        let f = e.tick_transitions(mid);
        // first frame of the new fade equals the in-flight colour: no jump
        assert!(
            f.is_empty() || f[0].1 == in_flight,
            "{f:?} vs {in_flight:?}"
        );
        let end = e.tick_transitions(mid + Duration::from_millis(1000));
        assert_eq!(end[0].1, RgbColor { r: 0, g: 255, b: 0 });
    }

    fn rgb_state_color(r: u8, g: u8, b: u8) -> RgbColor {
        RgbColor { r, g, b }
    }

    #[test]
    fn capabilities_are_enforced_and_uncontrollable_zones_skipped() {
        let mut e = LightingEngine::new();
        let ok = ZoneInfo::controllable("light_ram", "RAM", "x", "smbus", 10);
        let mut no_kelvin = ZoneInfo::controllable("light_fans", "Fans", "x", "usb", 8);
        no_kelvin.capabilities.kelvin = false;
        let dead = ZoneInfo::uncontrollable(
            "light_storage",
            "SSD",
            "samsung",
            "nvme",
            "no_public_protocol",
        );
        let skipped = e.register_zones(&[ok, no_kelvin, dead]);
        assert_eq!(skipped, vec!["light_storage".to_string()]);
        assert!(e.zone_state("light_storage").is_none());
        e.apply_update("light_fans", None, None, None, Some(2000), Some(0));
        assert_ne!(
            e.zone_state("light_fans").map(|s| s.color_temp_kelvin),
            Some(2000)
        );
        assert!(
            e.apply_update("light_gpu", Some(true), None, None, None, None)
                .is_empty()
        );
    }

    #[test]
    fn first_update_always_reaches_hardware_even_if_unchanged() {
        let mut e = engine_with("light_ram");
        let t0 = Instant::now();
        let d = LightState::default();
        let target = Some(d.rgb);
        e.apply_update_at(
            t0,
            "light_ram",
            Some(true),
            Some(d.brightness),
            target,
            None,
            Some(0),
        );
        assert_eq!(e.tick_transitions(t0).len(), 1);
        e.apply_update_at(
            t0,
            "light_ram",
            Some(true),
            Some(d.brightness),
            target,
            None,
            Some(0),
        );
        assert!(e.tick_transitions(t0).is_empty());
    }
}
