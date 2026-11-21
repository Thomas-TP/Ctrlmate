//! Preset-based cooling engine with non-bypassable Thermal Safety Watchdog.

use ctrlmate_protocol::CoolingPreset;
use serde::{Deserialize, Serialize};

/// Absolute hard ceilings preventing dangerous thermal configurations.
pub const MAX_ALLOWED_CPU_CRITICAL_C: f64 = 95.0;
pub const MAX_ALLOWED_GPU_CRITICAL_C: f64 = 88.0;
pub const MAX_ALLOWED_LIQUID_CRITICAL_C: f64 = 52.0;
pub const MIN_ALLOWED_AIO_PUMP_PERCENT: u8 = 50;

/// User-tunable thermal safety thresholds (always clamped to safe hardware limits).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermalSafetyConfig {
    pub cpu_critical_c: f64,
    pub gpu_critical_c: f64,
    pub liquid_critical_c: f64,
    /// Hysteresis drop (°C) before exiting emergency cooling override.
    pub recovery_hysteresis_c: f64,
    /// Minimum allowed duty cycle (%) for AIO watercooling pumps.
    pub min_aio_pump_percent: u8,
}

impl Default for ThermalSafetyConfig {
    fn default() -> Self {
        Self {
            cpu_critical_c: 88.0,
            gpu_critical_c: 83.0,
            liquid_critical_c: 48.0,
            recovery_hysteresis_c: 6.0,
            min_aio_pump_percent: 55,
        }
    }
}

impl ThermalSafetyConfig {
    /// Clamps any user input to hard physical safety limits.
    pub fn sanitized(mut self) -> Self {
        self.cpu_critical_c = self.cpu_critical_c.clamp(65.0, MAX_ALLOWED_CPU_CRITICAL_C);
        self.gpu_critical_c = self.gpu_critical_c.clamp(65.0, MAX_ALLOWED_GPU_CRITICAL_C);
        self.liquid_critical_c = self
            .liquid_critical_c
            .clamp(38.0, MAX_ALLOWED_LIQUID_CRITICAL_C);
        self.recovery_hysteresis_c = self.recovery_hysteresis_c.clamp(2.0, 15.0);
        self.min_aio_pump_percent = self.min_aio_pump_percent.clamp(MIN_ALLOWED_AIO_PUMP_PERCENT, 100);
        self
    }
}

/// Snapshot of critical thermal readings fed into the watchdog.
#[derive(Debug, Clone, Copy, Default)]
pub struct ThermalReadings {
    pub cpu_c: Option<f64>,
    pub gpu_c: Option<f64>,
    pub liquid_c: Option<f64>,
}

/// Target duty cycles resolved for a given cooling preset & watchdog state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoolingTargets {
    pub effective_preset: CoolingPreset,
    pub case_fan_percent: Option<u8>,
    pub aio_fan_percent: Option<u8>,
    pub aio_pump_percent: u8,
    pub gpu_fan_percent: Option<u8>,
    pub failsafe_triggered: bool,
}

/// Evaluates cooling presets while enforcing hardware thermal protection.
#[derive(Debug, Clone)]
pub struct CoolingWatchdog {
    config: ThermalSafetyConfig,
    requested_preset: CoolingPreset,
    failsafe_active: bool,
}

impl CoolingWatchdog {
    pub fn new(config: ThermalSafetyConfig) -> Self {
        Self {
            config: config.sanitized(),
            requested_preset: CoolingPreset::Auto,
            failsafe_active: false,
        }
    }

    pub fn config(&self) -> &ThermalSafetyConfig {
        &self.config
    }

    pub fn update_config(&mut self, config: ThermalSafetyConfig) {
        self.config = config.sanitized();
    }

    pub fn set_preset(&mut self, preset: CoolingPreset) {
        self.requested_preset = preset;
    }

    pub fn requested_preset(&self) -> CoolingPreset {
        self.requested_preset
    }

    pub fn is_failsafe_active(&self) -> bool {
        self.failsafe_active
    }

    /// Evaluates current temperatures and computes safe fan/pump targets.
    pub fn evaluate(&mut self, readings: ThermalReadings) -> CoolingTargets {
        let exceeded = readings
            .cpu_c
            .is_some_and(|t| t >= self.config.cpu_critical_c)
            || readings
                .gpu_c
                .is_some_and(|t| t >= self.config.gpu_critical_c)
            || readings
                .liquid_c
                .is_some_and(|t| t >= self.config.liquid_critical_c);

        if exceeded {
            self.failsafe_active = true;
        } else if self.failsafe_active {
            let h = self.config.recovery_hysteresis_c;
            let cpu_safe = readings
                .cpu_c
                .is_none_or(|t| t <= self.config.cpu_critical_c - h);
            let gpu_safe = readings
                .gpu_c
                .is_none_or(|t| t <= self.config.gpu_critical_c - h);
            let liquid_safe = readings
                .liquid_c
                .is_none_or(|t| t <= self.config.liquid_critical_c - h);

            if cpu_safe && gpu_safe && liquid_safe {
                self.failsafe_active = false;
            }
        }

        if self.failsafe_active {
            return CoolingTargets {
                effective_preset: CoolingPreset::FullSpeed,
                case_fan_percent: Some(100),
                aio_fan_percent: Some(100),
                aio_pump_percent: 100,
                gpu_fan_percent: Some(100),
                failsafe_triggered: true,
            };
        }

        let min_pump = self.config.min_aio_pump_percent;
        match self.requested_preset {
            CoolingPreset::Auto => CoolingTargets {
                effective_preset: CoolingPreset::Auto,
                case_fan_percent: None, // Hardware / BIOS auto curve
                aio_fan_percent: None,
                aio_pump_percent: 70.max(min_pump),
                gpu_fan_percent: None,
                failsafe_triggered: false,
            },
            CoolingPreset::Silent => CoolingTargets {
                effective_preset: CoolingPreset::Silent,
                case_fan_percent: Some(35),
                aio_fan_percent: Some(35),
                aio_pump_percent: min_pump,
                gpu_fan_percent: None,
                failsafe_triggered: false,
            },
            CoolingPreset::Balanced => CoolingTargets {
                effective_preset: CoolingPreset::Balanced,
                case_fan_percent: Some(55),
                aio_fan_percent: Some(55),
                aio_pump_percent: 75.max(min_pump),
                gpu_fan_percent: None,
                failsafe_triggered: false,
            },
            CoolingPreset::Performance => CoolingTargets {
                effective_preset: CoolingPreset::Performance,
                case_fan_percent: Some(80),
                aio_fan_percent: Some(80),
                aio_pump_percent: 90.max(min_pump),
                gpu_fan_percent: Some(80),
                failsafe_triggered: false,
            },
            CoolingPreset::FullSpeed => CoolingTargets {
                effective_preset: CoolingPreset::FullSpeed,
                case_fan_percent: Some(100),
                aio_fan_percent: Some(100),
                aio_pump_percent: 100,
                gpu_fan_percent: Some(100),
                failsafe_triggered: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pump_never_drops_below_hard_floor() {
        let cfg = ThermalSafetyConfig {
            min_aio_pump_percent: 10, // Unsafe value requested
            ..ThermalSafetyConfig::default()
        };
        let mut wd = CoolingWatchdog::new(cfg);
        wd.set_preset(CoolingPreset::Silent);

        let targets = wd.evaluate(ThermalReadings {
            cpu_c: Some(45.0),
            gpu_c: Some(40.0),
            liquid_c: Some(30.0),
        });

        assert!(!targets.failsafe_triggered);
        assert!(targets.aio_pump_percent >= MIN_ALLOWED_AIO_PUMP_PERCENT);
    }

    #[test]
    fn thermal_failsafe_overrides_silent_with_hysteresis() {
        let mut wd = CoolingWatchdog::new(ThermalSafetyConfig::default());
        wd.set_preset(CoolingPreset::Silent);

        // Exceeds GPU threshold (83°C)
        let hot = wd.evaluate(ThermalReadings {
            cpu_c: Some(70.0),
            gpu_c: Some(85.0),
            liquid_c: Some(36.0),
        });
        assert!(hot.failsafe_triggered);
        assert_eq!(hot.effective_preset, CoolingPreset::FullSpeed);
        assert_eq!(hot.aio_pump_percent, 100);

        // Still inside hysteresis band (80°C > 83 - 6 = 77°C)
        let warm = wd.evaluate(ThermalReadings {
            cpu_c: Some(65.0),
            gpu_c: Some(80.0),
            liquid_c: Some(35.0),
        });
        assert!(warm.failsafe_triggered);

        // Cooled below hysteresis (76°C <= 77°C) -> recovers to Silent
        let cool = wd.evaluate(ThermalReadings {
            cpu_c: Some(60.0),
            gpu_c: Some(76.0),
            liquid_c: Some(34.0),
        });
        assert!(!cool.failsafe_triggered);
        assert_eq!(cool.effective_preset, CoolingPreset::Silent);
    }
}
