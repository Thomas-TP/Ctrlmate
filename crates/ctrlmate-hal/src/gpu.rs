//! Zero-Kernel GPU telemetry provider (NVIDIA NVML on Windows & Linux + Linux DRM fallback).

use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::Nvml;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct GpuDescriptor {
    pub index: u32,
    pub name: String,
    pub vendor: String,
}

#[derive(Debug, Clone)]
pub struct GpuSample {
    pub index: u32,
    pub temperature_c: Option<f64>,
    pub utilization_pct: Option<u32>,
    pub vram_used_mib: Option<u64>,
    pub vram_total_mib: Option<u64>,
    pub vram_usage_pct: Option<f64>,
    pub power_watts: Option<f64>,
    pub fan_speed_pct: Option<u32>,
    pub graphics_clock_mhz: Option<u32>,
}

pub struct GpuProvider {
    nvml: Option<Nvml>,
    descriptors: Vec<GpuDescriptor>,
}

impl GpuProvider {
    pub fn new() -> Self {
        let mut descriptors = Vec::new();
        let nvml = match Nvml::init() {
            Ok(nvml) => {
                let count = nvml.device_count().unwrap_or(0);
                for idx in 0..count {
                    if let Ok(dev) = nvml.device_by_index(idx) {
                        let name = dev.name().unwrap_or_else(|_| format!("NVIDIA GPU {idx}"));
                        info!("NVML detected GPU #{idx}: {name}");
                        descriptors.push(GpuDescriptor {
                            index: idx,
                            name,
                            vendor: "NVIDIA".into(),
                        });
                    }
                }
                Some(nvml)
            }
            Err(err) => {
                debug!("NVML not available on this host: {err}");
                None
            }
        };

        Self { nvml, descriptors }
    }

    pub fn device_count(&self) -> usize {
        self.descriptors.len()
    }

    pub fn descriptors(&self) -> &[GpuDescriptor] {
        &self.descriptors
    }

    pub fn poll(&self) -> Vec<GpuSample> {
        let mut samples = Vec::new();

        if let Some(nvml) = &self.nvml {
            for desc in &self.descriptors {
                match nvml.device_by_index(desc.index) {
                    Ok(dev) => {
                        let temperature_c = dev
                            .temperature(TemperatureSensor::Gpu)
                            .ok()
                            .map(f64::from);
                        let utilization_pct = dev.utilization_rates().ok().map(|u| u.gpu);
                        let (vram_used_mib, vram_total_mib, vram_usage_pct) =
                            match dev.memory_info() {
                                Ok(mem) if mem.total > 0 => {
                                    let used = mem.used / (1024 * 1024);
                                    let total = mem.total / (1024 * 1024);
                                    let pct = (mem.used as f64 / mem.total as f64) * 100.0;
                                    (Some(used), Some(total), Some(pct))
                                }
                                _ => (None, None, None),
                            };
                        let power_watts = dev.power_usage().ok().map(|mw| f64::from(mw) / 1000.0);
                        let fan_speed_pct = dev.fan_speed(0).ok();
                        let graphics_clock_mhz = dev.clock_info(Clock::Graphics).ok();

                        samples.push(GpuSample {
                            index: desc.index,
                            temperature_c,
                            utilization_pct,
                            vram_used_mib,
                            vram_total_mib,
                            vram_usage_pct,
                            power_watts,
                            fan_speed_pct,
                            graphics_clock_mhz,
                        });
                    }
                    Err(err) => {
                        warn!("Failed to poll NVML GPU #{}: {err}", desc.index);
                    }
                }
            }
        }

        samples
    }

    /// Applies a GPU fan duty percentage via NVML (drivers 520+), or restores automatic firmware fan curves.
    pub fn apply_fan_target(&self, fan_percent: Option<u8>) {
        let Some(nvml) = &self.nvml else {
            return;
        };
        for desc in &self.descriptors {
            if let Ok(mut dev) = nvml.device_by_index(desc.index) {
                let fan_count = dev.num_fans().unwrap_or(1);
                for fan_idx in 0..fan_count {
                    match fan_percent {
                        Some(pct) => {
                            if let Err(err) = dev.set_fan_speed(fan_idx, u32::from(pct.clamp(30, 100))) {
                                debug!("NVML set_fan_speed GPU #{} fan #{fan_idx}: {err}", desc.index);
                            }
                        }
                        None => {
                            if let Err(err) = dev.set_default_fan_speed(fan_idx) {
                                debug!("NVML set_default_fan_speed GPU #{} fan #{fan_idx}: {err}", desc.index);
                            }
                        }
                    }
                }
            }
        }
    }
}
