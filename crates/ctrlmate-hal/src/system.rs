use ctrlmate_protocol::{HostDeviceInfo, SecurityMode};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, Networks, ProcessRefreshKind, RefreshKind, System};

pub struct SystemProvider {
    sys: System,
    networks: Networks,
}

impl SystemProvider {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything())
                .with_processes(ProcessRefreshKind::nothing()),
        );
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let networks = Networks::new_with_refreshed_list();
        Self { sys, networks }
    }

    pub fn refresh(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
    }

    pub fn sys(&self) -> &System {
        &self.sys
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

    pub fn cpu_frequency_mhz(&self) -> u64 {
        let cpus = self.sys.cpus();
        if cpus.is_empty() {
            return 0;
        }
        let sum: u64 = cpus.iter().map(|c| c.frequency()).sum();
        sum / (cpus.len() as u64)
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
            let traffic = data.total_received().saturating_add(data.total_transmitted());
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

/// Reads native Linux CPU package temperature from `/sys/class/hwmon` (`k10temp`, `coretemp`, `zenpower`).
#[cfg(target_os = "linux")]
pub fn read_linux_cpu_temp_c() -> Option<f64> {
    let entries = std::fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = std::fs::read_to_string(dir.join("name")).ok()?;
        let name = name.trim();
        if matches!(name, "k10temp" | "coretemp" | "zenpower" | "cpu_thermal") {
            if let Ok(raw) = std::fs::read_to_string(dir.join("temp1_input")) {
                if let Ok(milli_c) = raw.trim().parse::<f64>() {
                    return Some(milli_c / 1000.0);
                }
            }
        }
    }
    None
}
