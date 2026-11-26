//! Cooling policy exposed in the snapshot (architecture contract section 6).
//! The daemon is the only source of truth; the UI renders exactly this.

use crate::CoolingPreset;
use serde::{Deserialize, Serialize};

/// One point of a fan curve: at `temp_c` the fans run at `duty` percent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FanCurvePoint {
    pub temp_c: f64,
    /// Duty cycle, 0-100.
    pub duty: u8,
}

/// Fan curve + pump floor of one user preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetCurve {
    pub preset: CoolingPreset,
    /// Ascending by `temp_c`.
    pub fan_curve: Vec<FanCurvePoint>,
    /// Minimum pump duty for this preset, never below [`MIN_PUMP_PERCENT`].
    pub pump_floor: u8,
}

/// Non-bypassable watchdog thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThermalThresholds {
    pub cpu_critical_c: f64,
    pub gpu_critical_c: f64,
    pub liquid_critical_c: f64,
    /// All known sensors must be this far below their critical value to leave failsafe.
    pub hysteresis_c: f64,
    /// Coolant temperature at which the pump floor protection engages.
    pub pump_floor_c: f64,
}

impl Default for ThermalThresholds {
    fn default() -> Self {
        Self {
            cpu_critical_c: 93.0,
            gpu_critical_c: 88.0,
            liquid_critical_c: 55.0,
            hysteresis_c: 5.0,
            pump_floor_c: 35.0,
        }
    }
}

/// Effective state of the watchdog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CoolingState {
    #[default]
    Normal,
    /// Emergency: fans and pump forced to the failsafe floor or 100 %.
    Failsafe,
}

/// Why the watchdog is not in `normal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoolingReason {
    Temperature,
    SensorLost,
    WriteFailed,
}

/// The pump never runs below this duty (percent).
pub const MIN_PUMP_PERCENT: u8 = 50;

/// Complete cooling policy as seen by the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoolingPolicy {
    pub presets: Vec<PresetCurve>,
    pub thresholds: ThermalThresholds,
    /// Preset chosen by the user or Home Assistant.
    pub requested_preset: CoolingPreset,
    /// Preset actually applied (`gaming` replaces `auto` while a game runs).
    pub effective_preset: CoolingPreset,
    pub state: CoolingState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<CoolingReason>,
}

fn pts(v: &[(f64, u8)]) -> Vec<FanCurvePoint> {
    v.iter()
        .map(|&(temp_c, duty)| FanCurvePoint { temp_c, duty })
        .collect()
}

impl CoolingPolicy {
    /// Built-in curves sized for a Ryzen 9 7900X3D + Kraken 360 build.
    pub fn builtin() -> Self {
        Self {
            presets: vec![
                PresetCurve {
                    preset: CoolingPreset::Auto,
                    fan_curve: pts(&[
                        (30.0, 30),
                        (50.0, 40),
                        (62.0, 55),
                        (72.0, 75),
                        (82.0, 95),
                        (90.0, 100),
                    ]),
                    pump_floor: 60,
                },
                PresetCurve {
                    preset: CoolingPreset::Silent,
                    fan_curve: pts(&[
                        (30.0, 25),
                        (50.0, 30),
                        (65.0, 40),
                        (75.0, 60),
                        (85.0, 85),
                        (92.0, 100),
                    ]),
                    pump_floor: 50,
                },
                PresetCurve {
                    preset: CoolingPreset::Balanced,
                    fan_curve: pts(&[
                        (30.0, 30),
                        (50.0, 40),
                        (60.0, 55),
                        (70.0, 70),
                        (80.0, 90),
                        (88.0, 100),
                    ]),
                    pump_floor: 60,
                },
                PresetCurve {
                    preset: CoolingPreset::Performance,
                    fan_curve: pts(&[(30.0, 40), (45.0, 55), (55.0, 70), (65.0, 85), (75.0, 100)]),
                    pump_floor: 80,
                },
                PresetCurve {
                    preset: CoolingPreset::Gaming,
                    fan_curve: pts(&[(30.0, 45), (50.0, 60), (60.0, 75), (70.0, 90), (78.0, 100)]),
                    pump_floor: 90,
                },
            ],
            thresholds: ThermalThresholds::default(),
            requested_preset: CoolingPreset::Auto,
            effective_preset: CoolingPreset::Auto,
            state: CoolingState::Normal,
            reason: None,
        }
    }
}

impl Default for CoolingPolicy {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_policy_respects_safety_invariants() {
        let p = CoolingPolicy::builtin();
        assert_eq!(p.presets.len(), CoolingPreset::all().len());
        for c in &p.presets {
            assert!(c.pump_floor >= MIN_PUMP_PERCENT, "{:?}", c.preset);
            assert!(c.fan_curve.windows(2).all(|w| w[0].temp_c < w[1].temp_c));
            assert!(c.fan_curve.windows(2).all(|w| w[0].duty <= w[1].duty));
            assert!(c.fan_curve.iter().all(|pt| pt.duty <= 100));
            assert_eq!(c.fan_curve.last().map(|p| p.duty), Some(100));
        }
        assert!(p.thresholds.cpu_critical_c >= 85.0 && p.thresholds.cpu_critical_c <= 98.0);
    }

    #[test]
    fn policy_roundtrips_and_omits_empty_reason() {
        let p = CoolingPolicy::builtin();
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("reason").is_none());
        assert_eq!(v["state"], "normal");
        let back: CoolingPolicy = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
    }
}
