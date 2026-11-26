//! Samsung 990 PRO **with Heatsink** NVMe LED: read-only diagnostics, never controllable.
//!
//! * The heatsink LED is not exposed through any public, verified protocol: Samsung documents no
//!   command for it, and no open-source project we could verify (OpenRGB, liquidctl) implements one.
//!   Sending unverified NVMe vendor-specific admin opcodes to a boot drive is a data-safety risk, so
//!   this module only *detects* the drive through read-only storage descriptors
//!   (`IOCTL_STORAGE_QUERY_PROPERTY` / `/sys/class/nvme/*/model`) and reports the LED as
//!   `controllable: false` with the reason code `no_public_protocol`. It never registers a zone that
//!   can receive frames.
//! * Candidate detection is limited to the Heatsink variant: the plain 990 PRO and the 980 PRO have
//!   no LED, so they are not reported.

use ctrlmate_protocol::RgbDeviceStatus;

#[derive(Debug, Clone)]
pub struct DetectedSamsungNvme {
    pub model: String,
    pub firmware: Option<String>,
    pub device_path: String,
}

/// Probes read-only storage descriptors for Samsung 990 PRO / Heatsink NVMe SSDs.
pub fn detect_samsung_nvme_led_status() -> Vec<RgbDeviceStatus> {
    let drives = enumerate_samsung_nvme_drives();
    drives
        .into_iter()
        .enumerate()
        .map(|(idx, drv)| {
            let fw_suffix = drv
                .firmware
                .as_deref()
                .map(|f| format!(" (FW {f})"))
                .unwrap_or_default();
            RgbDeviceStatus {
                id: format!("samsung_nvme_led_{idx}"),
                name: format!("{}{fw_suffix} — LED Heatsink", drv.model),
                zone_id: "light_storage".into(),
                transport: "NVMe (read-only detection)".into(),
                controllable: false,
                reason: "no_public_protocol".into(),
            }
        })
        .collect()
}

/// `true` only for the "Samsung SSD 990 PRO with Heatsink" model string.
pub fn is_samsung_heatsink_candidate(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("samsung") && lower.contains("990 pro") && lower.contains("heatsink")
}

/// Zones for the Samsung LED: always informational (`controllable == false`).
pub fn samsung_zones() -> Vec<crate::zones::ZoneInfo> {
    detect_samsung_nvme_led_status()
        .into_iter()
        .map(|s| {
            crate::zones::ZoneInfo::uncontrollable(
                crate::zones::ZONE_STORAGE,
                s.name,
                "samsung_nvme_led",
                s.transport,
                "no_public_protocol",
            )
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn enumerate_samsung_nvme_drives() -> Vec<DetectedSamsungNvme> {
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR,
        STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
    };
    use windows::core::PCWSTR;

    let mut found = Vec::new();
    for idx in 0..8 {
        let path_str = format!(r"\\.\PhysicalDrive{idx}");
        let wide: Vec<u16> = OsStr::new(&path_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

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
        let Ok(handle) = handle else { continue };
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }

        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut buffer = [0u8; 1024];
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

        if ok.is_err() || (returned as usize) < size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
            continue;
        }
        // SAFETY: `buffer` holds at least size_of::<STORAGE_DEVICE_DESCRIPTOR>() initialised bytes;
        // read_unaligned copes with the byte buffer's alignment.
        let desc = unsafe {
            std::ptr::read_unaligned(buffer.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR)
        };
        let prod = read_cstr_at(&buffer[..returned as usize], desc.ProductIdOffset as usize);
        let rev = read_cstr_at(
            &buffer[..returned as usize],
            desc.ProductRevisionOffset as usize,
        );
        if let Some(model) = prod
            && is_samsung_heatsink_candidate(&model)
        {
            found.push(DetectedSamsungNvme {
                model,
                firmware: rev,
                device_path: path_str,
            });
        }
    }
    found
}

#[cfg(target_os = "windows")]
fn read_cstr_at(buf: &[u8], offset: usize) -> Option<String> {
    if offset == 0 || offset >= buf.len() {
        return None;
    }
    let slice = &buf[offset..];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    let s = String::from_utf8_lossy(&slice[..end]).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(target_os = "linux")]
fn enumerate_samsung_nvme_drives() -> Vec<DetectedSamsungNvme> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/nvme") else {
        return found;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Ok(model) = std::fs::read_to_string(dir.join("model")) else {
            continue;
        };
        let model = model.trim().to_string();
        if is_samsung_heatsink_candidate(&model) {
            let firmware = std::fs::read_to_string(dir.join("firmware_rev"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            found.push(DetectedSamsungNvme {
                model,
                firmware,
                device_path: format!("/dev/{}", entry.file_name().to_string_lossy()),
            });
        }
    }
    found
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn enumerate_samsung_nvme_drives() -> Vec<DetectedSamsungNvme> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samsung_status_is_never_controllable() {
        for z in samsung_zones() {
            assert!(!z.controllable);
        }
        for s in detect_samsung_nvme_led_status() {
            assert!(!s.controllable);
        }
    }

    #[test]
    fn matches_samsung_990_pro_model() {
        assert!(is_samsung_heatsink_candidate(
            "Samsung SSD 990 PRO with Heatsink 2TB"
        ));
        assert!(!is_samsung_heatsink_candidate("Samsung SSD 990 PRO 2TB"));
        assert!(!is_samsung_heatsink_candidate(
            "Samsung SSD 980 PRO with Heatsink 1TB"
        ));
        assert!(!is_samsung_heatsink_candidate("WD_BLACK SN850X 2TB"));
    }
}
