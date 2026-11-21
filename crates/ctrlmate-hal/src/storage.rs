//! Native storage & NVMe temperature telemetry (Win32 IOCTL_STORAGE_QUERY_PROPERTY / Linux hwmon).

use sysinfo::Disks;

#[derive(Debug, Clone)]
pub struct DriveDescriptor {
    pub id: String,
    pub name: String,
    pub manufacturer: Option<String>,
    pub has_temperature: bool,
}

#[derive(Debug, Clone)]
pub struct DriveSample {
    pub id: String,
    pub used_percent: f64,
    pub temperature_c: Option<f64>,
}

pub fn enumerate_drives() -> Vec<DriveDescriptor> {
    let disks = Disks::new_with_refreshed_list();
    let temps = read_physical_drive_temperatures();
    let mut result = Vec::new();

    for (idx, disk) in disks.list().iter().enumerate() {
        let total = disk.total_space();
        if total < 1_000_000_000 {
            continue;
        }
        let mount = disk.mount_point().to_string_lossy();
        let disk_name = disk.name().to_string_lossy();
        let label = if disk_name.trim().is_empty() {
            format!("Storage ({mount})")
        } else {
            format!("{} ({mount})", disk_name.trim())
        };
        let temp_match = temps.get(idx).and_then(|(_, t)| *t).is_some();
        let model_override = temps.get(idx).and_then(|(m, _)| m.clone());
        let final_name = model_override.unwrap_or(label);

        result.push(DriveDescriptor {
            id: format!("storage_{idx}"),
            name: final_name.clone(),
            manufacturer: infer_storage_vendor(&final_name),
            has_temperature: temp_match,
        });
    }

    result
}

pub fn poll_drives() -> Vec<DriveSample> {
    let disks = Disks::new_with_refreshed_list();
    let temps = read_physical_drive_temperatures();
    let mut samples = Vec::new();

    for (idx, disk) in disks.list().iter().enumerate() {
        let total = disk.total_space();
        if total < 1_000_000_000 {
            continue;
        }
        let used = total.saturating_sub(disk.available_space());
        let used_percent = (used as f64 / total as f64) * 100.0;
        let temperature_c = temps.get(idx).and_then(|(_, t)| *t);

        samples.push(DriveSample {
            id: format!("storage_{idx}"),
            used_percent,
            temperature_c,
        });
    }

    samples
}

fn infer_storage_vendor(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("samsung") {
        Some("Samsung".into())
    } else if lower.contains("wd") || lower.contains("western digital") {
        Some("Western Digital".into())
    } else if lower.contains("crucial") || lower.contains("micron") {
        Some("Crucial".into())
    } else if lower.contains("kingston") {
        Some("Kingston".into())
    } else if lower.contains("sabrent") {
        Some("Sabrent".into())
    } else if lower.contains("corsair") {
        Some("Corsair".into())
    } else {
        None
    }
}

/// Queries physical drive temperatures via native Win32 `IOCTL_STORAGE_QUERY_PROPERTY`
/// (Zero kernel driver; works natively on NVMe SSDs like Samsung 990 PRO).
#[cfg(target_os = "windows")]
fn read_physical_drive_temperatures() -> Vec<(Option<String>, Option<f64>)> {
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceTemperatureProperty, IOCTL_STORAGE_QUERY_PROPERTY,
        STORAGE_PROPERTY_QUERY, STORAGE_TEMPERATURE_DATA_DESCRIPTOR,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    let mut out = Vec::new();

    for drive_idx in 0..8 {
        let path_str = format!(r"\\.\PhysicalDrive{drive_idx}");
        let wide: Vec<u16> = OsStr::new(&path_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // Opening with 0 access (query metadata/properties) often works without full raw disk write access;
        // if elevated as Windows Service it succeeds reliably for all NVMe drives.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };

        let Ok(handle) = handle else {
            continue;
        };
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }

        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceTemperatureProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };

        let mut buffer = [0u8; 512];
        let mut returned: u32 = 0;

        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(&query as *const _ as *const std::ffi::c_void),
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                Some(buffer.as_mut_ptr() as *mut std::ffi::c_void),
                buffer.len() as u32,
                Some(&mut returned),
                None,
            )
        };

        unsafe {
            let _ = CloseHandle(handle);
        }

        let temp_c = if ok.is_ok()
            && (returned as usize) >= size_of::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>()
        {
            let desc = unsafe { &*(buffer.as_ptr() as *const STORAGE_TEMPERATURE_DATA_DESCRIPTOR) };
            let raw = desc.TemperatureInfo[0].Temperature;
            if (-20..=120).contains(&raw) {
                Some(f64::from(raw))
            } else {
                None
            }
        } else {
            None
        };

        out.push((None, temp_c));
    }

    out
}

#[cfg(target_os = "linux")]
fn read_physical_drive_temperatures() -> Vec<(Option<String>, Option<f64>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Ok(name) = std::fs::read_to_string(dir.join("name")) else {
            continue;
        };
        if name.trim() == "nvme" {
            let model = std::fs::read_to_string(dir.join("device/model"))
                .ok()
                .map(|s| s.trim().to_string());
            let temp = std::fs::read_to_string(dir.join("temp1_input"))
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .map(|mc| mc / 1000.0);
            out.push((model, temp));
        }
    }
    out
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn read_physical_drive_temperatures() -> Vec<(Option<String>, Option<f64>)> {
    Vec::new()
}
