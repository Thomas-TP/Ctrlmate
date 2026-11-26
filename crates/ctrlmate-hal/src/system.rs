use ctrlmate_protocol::{HostDeviceInfo, SecurityMode};
use std::path::Path;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, Networks, RefreshKind, System};

pub struct SystemProvider {
    sys: System,
    networks: Networks,
}

impl Default for SystemProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemProvider {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let networks = Networks::new_with_refreshed_list();
        Self { sys, networks }
    }

    pub fn refresh(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
    }

    pub fn cpu_brand(&self) -> String {
        self.sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "CPU".into())
    }

    pub fn global_cpu_usage(&self) -> f32 {
        self.sys.global_cpu_usage()
    }

    /// Mean per-core frequency reported by the OS, `None` when it reports nothing.
    ///
    /// Note: on Windows the OS value is the *nominal / processor-information* frequency and does not
    /// follow boost clocks (Precision Boost, X3D); it is a coarse indicator, not a clock measurement.
    /// Linux reads `cpufreq` and is accurate.
    pub fn cpu_frequency_mhz(&self) -> Option<u64> {
        let cpus = self.sys.cpus();
        if cpus.is_empty() {
            return None;
        }
        let sum: u64 = cpus.iter().map(|c| c.frequency()).sum();
        let mean = sum / (cpus.len() as u64);
        (mean > 0).then_some(mean)
    }

    pub fn total_memory_bytes(&self) -> u64 {
        self.sys.total_memory()
    }

    pub fn used_memory_bytes(&self) -> u64 {
        self.sys.used_memory()
    }

    pub fn memory_usage_percent(&self) -> f64 {
        let total = self.sys.total_memory();
        if total == 0 {
            return 0.0;
        }
        (self.sys.used_memory() as f64 / total as f64) * 100.0
    }

    pub fn primary_mac_address(&self) -> Option<String> {
        let mut best: Option<(u64, String)> = None;
        for (name, data) in &self.networks {
            let lower = name.to_ascii_lowercase();
            if lower.contains("loopback")
                || lower.contains("docker")
                || lower.contains("vethernet")
                || lower.contains("vmware")
                || lower.contains("vmnet")
                || lower.contains("virtual")
                || lower.contains("vbox")
                || lower.contains("bluetooth")
                || lower.contains("tailscale")
                || lower.contains("zerotier")
                || lower.contains("wireguard")
                || lower.contains("wg")
            {
                continue;
            }
            let mac = data.mac_address();
            if mac.0 == [0; 6] {
                continue;
            }
            // Skip well-known hypervisor/virtual OUI prefixes (VMware, VirtualBox, Hyper-V)
            if matches!(
                (mac.0[0], mac.0[1], mac.0[2]),
                (0x00, 0x50, 0x56) | (0x00, 0x0C, 0x29) | (0x08, 0x00, 0x27) | (0x00, 0x15, 0x5D)
            ) {
                continue;
            }
            let traffic = data
                .total_received()
                .saturating_add(data.total_transmitted());
            let mac_str = mac.to_string();
            match &best {
                Some((best_traffic, _)) if *best_traffic >= traffic => {}
                _ => best = Some((traffic, mac_str)),
            }
        }
        best.map(|(_, mac)| mac)
    }

    pub fn host_info(
        &self,
        device_id: &str,
        sw_version: &str,
        security_mode: SecurityMode,
    ) -> HostDeviceInfo {
        let hostname = System::host_name().unwrap_or_else(|| "Ctrlmate-PC".into());
        let os_name = System::name().unwrap_or_else(|| std::env::consts::OS.into());
        let os_version = os_version_string();

        HostDeviceInfo {
            device_id: device_id.to_string(),
            name: hostname,
            manufacturer: "Ctrlmate".into(),
            model: format!("{os_name} PC"),
            os_name,
            os_version,
            sw_version: sw_version.to_string(),
            mac_address: self.primary_mac_address(),
            security_mode,
        }
    }
}

pub fn os_version_string() -> String {
    System::long_os_version()
        .or_else(System::os_version)
        .unwrap_or_else(|| std::env::consts::OS.into())
}

pub fn infer_cpu_vendor(brand: &str) -> Option<String> {
    let lower = brand.to_ascii_lowercase();
    if lower.contains("amd") || lower.contains("ryzen") || lower.contains("epyc") {
        Some("AMD".into())
    } else if lower.contains("intel") || lower.contains("core") || lower.contains("xeon") {
        Some("Intel".into())
    } else {
        None
    }
}

/// CPU temperatures from hwmon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuTemps {
    /// Tctl / Tdie on AMD, `Package id 0` on Intel, else the first sensor of the CPU driver.
    pub package_c: Option<f64>,
    /// `(label, C)` per CCD (`Tccd1`, ...).
    pub ccd_c: Vec<(String, f64)>,
}

/// Reads CPU temperatures below `hwmon_root` (`/sys/class/hwmon`). One unreadable hwmon directory
/// never aborts the scan of the others.
pub fn read_cpu_temps_from(hwmon_root: &Path) -> CpuTemps {
    let mut out = CpuTemps::default();
    let Ok(entries) = std::fs::read_dir(hwmon_root) else {
        return out;
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let Ok(name) = std::fs::read_to_string(dir.join("name")) else {
            continue;
        };
        if !matches!(
            name.trim(),
            "k10temp" | "coretemp" | "zenpower" | "cpu_thermal"
        ) {
            continue;
        }
        let mut first: Option<f64> = None;
        for i in 1..=24 {
            let Ok(raw) = std::fs::read_to_string(dir.join(format!("temp{i}_input"))) else {
                continue;
            };
            let Ok(milli) = raw.trim().parse::<f64>() else {
                continue;
            };
            let c = milli / 1000.0;
            let label = std::fs::read_to_string(dir.join(format!("temp{i}_label")))
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
            let lower = label.to_ascii_lowercase();
            first.get_or_insert(c);
            if lower.starts_with("tccd") {
                out.ccd_c.push((label, c));
            } else if (lower == "tctl" || lower == "tdie" || lower.starts_with("package"))
                && out.package_c.is_none()
            {
                out.package_c = Some(c);
            }
        }
        if out.package_c.is_none() {
            out.package_c = first;
        }
    }
    out
}

/// Native Linux CPU temperatures (`k10temp`, `coretemp`, `zenpower`, ...).
#[cfg(target_os = "linux")]
pub fn read_linux_cpu_temps() -> CpuTemps {
    read_cpu_temps_from(Path::new("/sys/class/hwmon"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn k10temp_with_ccds_and_a_broken_sibling_directory() {
        let root = std::env::temp_dir().join(format!("ctrlmate-hal-cpu-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // hwmon0: unreadable name (the old `?` aborted the whole loop here)
        fs::create_dir_all(root.join("hwmon0")).unwrap();
        let k = root.join("hwmon1");
        fs::create_dir_all(&k).unwrap();
        for (file, content) in [
            ("name", "k10temp"),
            ("temp1_input", "71500"),
            ("temp1_label", "Tctl"),
            ("temp3_input", "64000"),
            ("temp3_label", "Tccd1"),
            ("temp4_input", "66250"),
            ("temp4_label", "Tccd2"),
        ] {
            fs::write(k.join(file), format!("{content}\n")).unwrap();
        }
        let t = read_cpu_temps_from(&root);
        assert_eq!(t.package_c, Some(71.5));
        assert_eq!(
            t.ccd_c,
            vec![("Tccd1".to_string(), 64.0), ("Tccd2".to_string(), 66.25)]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_hwmon_means_no_temperature() {
        assert_eq!(
            read_cpu_temps_from(Path::new("/definitely/not/here")),
            CpuTemps::default()
        );
    }

    #[test]
    fn vendor_inference() {
        assert_eq!(
            infer_cpu_vendor("AMD Ryzen 9 7900X3D").as_deref(),
            Some("AMD")
        );
        assert_eq!(
            infer_cpu_vendor("Intel(R) Core(TM) i9").as_deref(),
            Some("Intel")
        );
        assert_eq!(infer_cpu_vendor("Mystery"), None);
    }
}
