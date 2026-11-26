//! GPU telemetry and fan control.
//!
//! * NVIDIA: NVML (Windows and Linux), user-mode only.
//! * AMD on Linux: real `amdgpu` hwmon / DRM sysfs reads (temperature, power, fan, busy %, VRAM).
//!   There is no fan control for AMD here: the firmware keeps the fans.
//!
//! Nothing is fabricated: a value the driver does not provide is `None`.

use anyhow::{Result, bail};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct GpuDescriptor {
    pub index: u32,
    pub name: String,
    pub vendor: String,
}

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone)]
enum Backend {
    Nvml {
        nvml_index: u32,
    },
    /// `hwmon` directory + DRM `device` directory.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Amdgpu {
        hwmon: PathBuf,
        device: PathBuf,
    },
}

pub struct GpuProvider {
    nvml: Option<Nvml>,
    descriptors: Vec<GpuDescriptor>,
    backends: Vec<Backend>,
}

impl Default for GpuProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuProvider {
    pub fn new() -> Self {
        let mut descriptors = Vec::new();
        let mut backends = Vec::new();
        let nvml = match Nvml::init() {
            Ok(nvml) => {
                let count = nvml.device_count().unwrap_or(0);
                for idx in 0..count {
                    if let Ok(dev) = nvml.device_by_index(idx) {
                        let name = dev.name().unwrap_or_else(|_| format!("NVIDIA GPU {idx}"));
                        info!("NVML detected GPU #{idx}: {name}");
                        descriptors.push(GpuDescriptor {
                            index: descriptors.len() as u32,
                            name,
                            vendor: "NVIDIA".into(),
                        });
                        backends.push(Backend::Nvml { nvml_index: idx });
                    }
                }
                Some(nvml)
            }
            Err(err) => {
                debug!("NVML not available on this host: {err}");
                None
            }
        };

        #[cfg(target_os = "linux")]
        for (hwmon, device, name) in find_amdgpus(Path::new("/sys")) {
            info!("amdgpu detected: {name}");
            descriptors.push(GpuDescriptor {
                index: descriptors.len() as u32,
                name,
                vendor: "AMD".into(),
            });
            backends.push(Backend::Amdgpu { hwmon, device });
        }

        Self {
            nvml,
            descriptors,
            backends,
        }
    }

    pub fn device_count(&self) -> usize {
        self.descriptors.len()
    }

    pub fn descriptors(&self) -> &[GpuDescriptor] {
        &self.descriptors
    }

    pub fn poll(&self) -> Vec<GpuSample> {
        let mut samples = Vec::new();
        for (desc, backend) in self.descriptors.iter().zip(&self.backends) {
            match backend {
                Backend::Nvml { nvml_index } => {
                    let Some(nvml) = &self.nvml else { continue };
                    match nvml.device_by_index(*nvml_index) {
                        Ok(dev) => {
                            let (vram_used_mib, vram_total_mib, vram_usage_pct) =
                                match dev.memory_info() {
                                    Ok(mem) if mem.total > 0 => (
                                        Some(mem.used / (1024 * 1024)),
                                        Some(mem.total / (1024 * 1024)),
                                        Some(mem.used as f64 / mem.total as f64 * 100.0),
                                    ),
                                    _ => (None, None, None),
                                };
                            samples.push(GpuSample {
                                index: desc.index,
                                temperature_c: dev
                                    .temperature(TemperatureSensor::Gpu)
                                    .ok()
                                    .map(f64::from),
                                utilization_pct: dev.utilization_rates().ok().map(|u| u.gpu),
                                vram_used_mib,
                                vram_total_mib,
                                vram_usage_pct,
                                power_watts: dev
                                    .power_usage()
                                    .ok()
                                    .map(|mw| f64::from(mw) / 1000.0),
                                fan_speed_pct: dev.fan_speed(0).ok(),
                                graphics_clock_mhz: dev.clock_info(Clock::Graphics).ok(),
                            });
                        }
                        Err(err) => warn!("Failed to poll NVML GPU #{}: {err}", desc.index),
                    }
                }
                Backend::Amdgpu { hwmon, device } => {
                    samples.push(read_amdgpu(desc.index, hwmon, device));
                }
            }
        }
        samples
    }

    /// Sets the GPU fans (NVML) to `fan_percent`, or hands them back to the firmware curve with
    /// `None`. Returns an error when any fan could not be written; NVIDIA only (AMD is a no-op).
    pub fn apply_fan_target(&self, fan_percent: Option<u8>) -> Result<()> {
        let Some(nvml) = &self.nvml else {
            return Ok(());
        };
        let mut failures = Vec::new();
        for (desc, backend) in self.descriptors.iter().zip(&self.backends) {
            let Backend::Nvml { nvml_index } = backend else {
                continue;
            };
            let Ok(mut dev) = nvml.device_by_index(*nvml_index) else {
                failures.push(format!("GPU #{} unavailable", desc.index));
                continue;
            };
            let fan_count = dev.num_fans().unwrap_or(1);
            for fan_idx in 0..fan_count {
                let r = match fan_percent {
                    Some(pct) => dev.set_fan_speed(fan_idx, u32::from(pct.clamp(30, 100))),
                    None => dev.set_default_fan_speed(fan_idx),
                };
                if let Err(err) = r {
                    failures.push(format!("GPU #{} fan #{fan_idx}: {err}", desc.index));
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("{}", failures.join("; "))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// amdgpu sysfs (pure file parsing, unit-testable on any OS)
// ---------------------------------------------------------------------------------------------

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_num(p: &Path) -> Option<f64> {
    read_trim(p)?.parse().ok()
}

/// Finds `(hwmon dir, DRM device dir, name)` of every amdgpu under `sys_root` (`/sys`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn find_amdgpus(sys_root: &Path) -> Vec<(PathBuf, PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(sys_root.join("class/hwmon")) else {
        return out;
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for hwmon in dirs {
        if read_trim(&hwmon.join("name")).as_deref() != Some("amdgpu") {
            continue;
        }
        let device = hwmon.join("device");
        let name = read_trim(&device.join("product_name"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "AMD Radeon GPU".to_string());
        out.push((hwmon, device, name));
    }
    out
}

fn read_amdgpu(index: u32, hwmon: &Path, device: &Path) -> GpuSample {
    let vram_used = read_num(&device.join("mem_info_vram_used"));
    let vram_total = read_num(&device.join("mem_info_vram_total")).filter(|t| *t > 0.0);
    // power1_average / power1_input are in microwatts.
    let power = read_num(&hwmon.join("power1_average"))
        .or_else(|| read_num(&hwmon.join("power1_input")))
        .map(|uw| uw / 1_000_000.0);
    // fan: pwm1 (0..255) is the duty; fan1_input is RPM and not a percentage, so it is not reported as %.
    let fan_pct = read_num(&hwmon.join("pwm1")).map(|v| (v / 255.0 * 100.0).round() as u32);
    GpuSample {
        index,
        // temp1 is the edge sensor; temp2 junction, temp3 memory. Edge matches NVML's "GPU" sensor.
        temperature_c: read_num(&hwmon.join("temp1_input")).map(|m| m / 1000.0),
        utilization_pct: read_num(&device.join("gpu_busy_percent")).map(|v| v.round() as u32),
        vram_used_mib: vram_used.map(|b| (b / 1_048_576.0) as u64),
        vram_total_mib: vram_total.map(|b| (b / 1_048_576.0) as u64),
        vram_usage_pct: match (vram_used, vram_total) {
            (Some(u), Some(t)) => Some(u / t * 100.0),
            _ => None,
        },
        power_watts: power,
        fan_speed_pct: fan_pct,
        // freq1_input (Hz) is the shader clock when exposed.
        graphics_clock_mhz: read_num(&hwmon.join("freq1_input"))
            .map(|hz| (hz / 1_000_000.0) as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn amdgpu_sysfs_is_parsed_and_gaps_stay_none() {
        let root = tempfile_dir("amd");
        let hw = root.join("class/hwmon/hwmon3");
        fs::create_dir_all(hw.join("device")).unwrap();
        fs::write(hw.join("name"), "amdgpu\n").unwrap();
        fs::write(hw.join("temp1_input"), "61000\n").unwrap();
        fs::write(hw.join("power1_average"), "187000000\n").unwrap();
        fs::write(hw.join("pwm1"), "128\n").unwrap();
        fs::write(hw.join("device/gpu_busy_percent"), "42\n").unwrap();
        fs::write(hw.join("device/mem_info_vram_total"), "8589934592\n").unwrap();
        fs::write(hw.join("device/mem_info_vram_used"), "2147483648\n").unwrap();
        // a non-GPU hwmon is ignored
        let other = root.join("class/hwmon/hwmon1");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("name"), "k10temp\n").unwrap();

        let found = find_amdgpus(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].2, "AMD Radeon GPU");
        let s = read_amdgpu(1, &found[0].0, &found[0].1);
        assert_eq!(s.temperature_c, Some(61.0));
        assert_eq!(s.utilization_pct, Some(42));
        assert_eq!(s.vram_total_mib, Some(8192));
        assert_eq!(s.vram_used_mib, Some(2048));
        assert_eq!(s.vram_usage_pct, Some(25.0));
        assert!((s.power_watts.unwrap() - 187.0).abs() < 1e-9);
        assert_eq!(s.fan_speed_pct, Some(50));
        assert_eq!(s.graphics_clock_mhz, None);
        let _ = fs::remove_dir_all(&root);
    }

    fn tempfile_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ctrlmate-hal-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
}
