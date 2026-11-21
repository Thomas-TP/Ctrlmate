//! Clean, calibrated PC lighting engine for Ctrlmate.
//!
//! Focuses on Brightness + RGB + Kelvin color temperature (`2000K..=6500K`)
//! with smooth 60 FPS transitions and LED gamma calibration so PC LEDs match
//! Philips Hue and Govee ambient lighting seamlessly.

use ctrlmate_protocol::{
    LightColorMode, LightState, MAX_COLOR_TEMP_KELVIN, MIN_COLOR_TEMP_KELVIN, RgbColor,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Converts a color temperature in Kelvin (`2000K..=6500K`) into a calibrated RGB triple.
///
/// Uses Tanner Helland's Planckian locus approximation followed by subtle white-balance
/// compensation suited for WS2812B / SK6812 PC ARGB LEDs (whose blue emitters tend to
/// dominate cold/neutral whites compared to Philips Hue phosphor LEDs).
pub fn kelvin_to_rgb(kelvin: u16) -> RgbColor {
    let clamped = f64::from(kelvin.clamp(MIN_COLOR_TEMP_KELVIN, MAX_COLOR_TEMP_KELVIN));
    let temp = clamped / 100.0;

    let red = if temp <= 66.0 {
        255.0
    } else {
        let r = 329.698_727_446 * (temp - 60.0).powf(-0.133_204_759_2);
        r.clamp(0.0, 255.0)
    };

    let green = if temp <= 66.0 {
        let g = 99.470_802_586_1 * temp.ln() - 161.119_568_166_1;
        g.clamp(0.0, 255.0)
    } else {
        let g = 288.122_169_528_3 * (temp - 60.0).powf(-0.075_514_849_2);
        g.clamp(0.0, 255.0)
    };

    let blue = if temp >= 66.0 {
        255.0
    } else if temp <= 19.0 {
        0.0
    } else {
        let b = 138.517_731_223_1 * (temp - 10.0).ln() - 305.044_792_730_7;
        b.clamp(0.0, 255.0)
    };

    // Subtle ARGB LED blue attenuation so warm whites (2200K-3200K) look truly warm
    // alongside Philips Hue and Govee bulbs.
    let calibrated_blue = (blue * 0.90).clamp(0.0, 255.0);

    RgbColor {
        r: red.round() as u8,
        g: green.round() as u8,
        b: calibrated_blue.round() as u8,
    }
}

/// Applies brightness scaling (0..=255) and optional perceptual LED gamma (gamma = 2.2)
/// to produce the final hardware RGB values sent to LED controllers.
pub fn render_hardware_rgb(state: &LightState, apply_gamma: bool) -> RgbColor {
    if !state.is_on || state.brightness == 0 {
        return RgbColor { r: 0, g: 0, b: 0 };
    }

    let base = match state.color_mode {
        LightColorMode::Rgb => state.rgb,
        LightColorMode::ColorTemp => kelvin_to_rgb(state.color_temp_kelvin),
    };

    let scale = f64::from(state.brightness) / 255.0;
    let map_channel = |ch: u8| -> u8 {
        let linear = (f64::from(ch) / 255.0) * scale;
        let out = if apply_gamma {
            linear.powf(2.2)
        } else {
            linear
        };
        (out * 255.0).round().clamp(0.0, 255.0) as u8
    };

    RgbColor {
        r: map_channel(base.r),
        g: map_channel(base.g),
        b: map_channel(base.b),
    }
}

/// Smooth ease-in-out cubic interpolation between two hardware RGB states.
#[derive(Debug, Clone)]
pub struct RgbTransition {
    from: RgbColor,
    to: RgbColor,
    started_at: Instant,
    duration: Duration,
}

impl RgbTransition {
    pub fn new(from: RgbColor, to: RgbColor, duration: Duration) -> Self {
        Self {
            from,
            to,
            started_at: Instant::now(),
            duration,
        }
    }

    pub fn sample(&self, now: Instant) -> (RgbColor, bool) {
        if self.duration.is_zero() {
            return (self.to, true);
        }

        let elapsed = now.saturating_duration_since(self.started_at);
        let t = (elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0);
        // Smoothstep ease-in-out
        let eased = t * t * (3.0 - 2.0 * t);

        let lerp = |a: u8, b: u8| -> u8 {
            (f64::from(a) + (f64::from(b) - f64::from(a)) * eased)
                .round()
                .clamp(0.0, 255.0) as u8
        };

        let color = RgbColor {
            r: lerp(self.from.r, self.to.r),
            g: lerp(self.from.g, self.to.g),
            b: lerp(self.from.b, self.to.b),
        };
        (color, t >= 1.0)
    }
}

/// Manages the global lighting master state and individual hardware lighting zones.
#[derive(Debug, Default)]
pub struct LightingEngine {
    zones: BTreeMap<String, LightState>,
    transitions: BTreeMap<String, RgbTransition>,
}

impl LightingEngine {
    pub fn new() -> Self {
        let mut zones = BTreeMap::new();
        zones.insert("light_global".into(), LightState::default());
        Self {
            zones,
            transitions: BTreeMap::new(),
        }
    }

    pub fn register_zone(&mut self, entity_id: impl Into<String>, initial: LightState) {
        self.zones.entry(entity_id.into()).or_insert(initial);
    }

    pub fn zone_state(&self, entity_id: &str) -> Option<&LightState> {
        self.zones.get(entity_id)
    }

    pub fn all_zones(&self) -> &BTreeMap<String, LightState> {
        &self.zones
    }

    /// Applies a partial light update to a zone (or cascades to all zones if `light_global`).
    /// Returns the list of affected `(entity_id, new_state)` pairs.
    pub fn apply_update(
        &mut self,
        entity_id: &str,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    ) -> Vec<(String, LightState)> {
        let target_ids: Vec<String> = if entity_id == "light_global" {
            self.zones.keys().cloned().collect()
        } else if self.zones.contains_key(entity_id) {
            vec![entity_id.to_string()]
        } else {
            return Vec::new();
        };

        let duration = Duration::from_millis(u64::from(transition_ms.unwrap_or(250)));
        let mut updated = Vec::with_capacity(target_ids.len());

        for id in target_ids {
            if let Some(state) = self.zones.get_mut(&id) {
                let prev_rgb = render_hardware_rgb(state, false);

                if let Some(on) = is_on {
                    state.is_on = on;
                }
                if let Some(b) = brightness {
                    state.brightness = b;
                    if b > 0 && is_on.is_none() {
                        state.is_on = true;
                    }
                }
                if let Some(k) = color_temp_kelvin {
                    let clamped = k.clamp(MIN_COLOR_TEMP_KELVIN, MAX_COLOR_TEMP_KELVIN);
                    state.color_temp_kelvin = clamped;
                    state.color_mode = LightColorMode::ColorTemp;
                    state.rgb = kelvin_to_rgb(clamped);
                } else if let Some(color) = rgb {
                    state.rgb = color;
                    state.color_mode = LightColorMode::Rgb;
                }

                let next_rgb = render_hardware_rgb(state, false);
                self.transitions
                    .insert(id.clone(), RgbTransition::new(prev_rgb, next_rgb, duration));
                updated.push((id, state.clone()));
            }
        }

        updated
    }

    /// Advances active 60 FPS transitions and returns hardware RGB frames to push to devices.
    pub fn tick_transitions(&mut self, now: Instant) -> Vec<(String, RgbColor)> {
        let mut frames = Vec::new();
        let mut completed = Vec::new();

        for (id, tr) in &self.transitions {
            let (rgb, done) = tr.sample(now);
            frames.push((id.clone(), rgb));
            if done {
                completed.push(id.clone());
            }
        }

        for id in completed {
            self.transitions.remove(&id);
        }

        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_kelvin_has_higher_red_than_blue() {
        let warm = kelvin_to_rgb(2700);
        let cool = kelvin_to_rgb(6500);

        assert_eq!(warm.r, 255);
        assert!(warm.r > warm.g && warm.g > warm.b);
        assert!(cool.b > warm.b);
    }

    #[test]
    fn off_state_renders_black() {
        let state = LightState {
            is_on: false,
            brightness: 255,
            ..LightState::default()
        };
        assert_eq!(
            render_hardware_rgb(&state, true),
            RgbColor { r: 0, g: 0, b: 0 }
        );
    }

    #[test]
    fn global_update_cascades_to_zones() {
        let mut engine = LightingEngine::new();
        engine.register_zone("light_motherboard", LightState::default());
        engine.register_zone("light_cooler", LightState::default());

        let updates = engine.apply_update(
            "light_global",
            Some(true),
            Some(128),
            None,
            Some(2700),
            Some(500),
        );

        assert_eq!(updates.len(), 3);
        for (_, st) in updates {
            assert_eq!(st.brightness, 128);
            assert_eq!(st.color_temp_kelvin, 2700);
            assert_eq!(st.color_mode, LightColorMode::ColorTemp);
        }
    }
}
