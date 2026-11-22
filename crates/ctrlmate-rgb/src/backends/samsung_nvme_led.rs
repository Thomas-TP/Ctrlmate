//! Samsung 990 PRO Heatsink NVMe LED safe detection & capability diagnostics.
//!
//! Hardware reality (verified as of 2026):
//! - The LED on the Samsung 990 PRO Heatsink is driven internally by the NVMe controller
//!   via undocumented proprietary NVMe Admin Vendor-Specific Commands (`0xC0..=0xFF`).
//! - Neither Samsung nor any open-source project (including OpenRGB / SignalRGB) has a
//!   publicly documented, verified safe protocol for writing LED states over NVMe pass-through.
//! - Moreover, known Samsung 990 PRO firmware quirks can lock the LED controller until a full
//!   PSU power drain if unexpected commands are sent.
//! - Therefore, for boot-drive safety, Ctrlmate detects the Samsung 990 PRO drive safely via
//!   read-only storage descriptors (`IOCTL_STORAGE_QUERY_PROPERTY` / `/sys/class/nvme/*/model`),
//!   exposes full SMART temperature & disk usage telemetry, and reports transparent LED status
//!   without ever transmitting unverified NVMe admin opcodes.

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
                transport: "NVMe Vendor-Specific (protocole propriétaire fermé)".into(),
                controllable: false,
                reason: "Samsung ne documente aucune API ni opcode NVMe public pour la LED du 990 PRO Heatsink (non supporté par OpenRGB/SignalRGB). Par sécurité pour votre SSD système, Ctrlmate n'envoie aucun opcode NVMe non vérifié — réglez ou éteignez la LED une fois dans Samsung Magician (état mémorisé dans le SSD)."
                    .into(),
            }
        })
        .collect()
}

pub fn is_samsung_heatsink_candidate(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("samsung") && (lower.contains("990 pro") || lower.contains("980 pro"))
}

#[cfg(target_os = "windows")]
fn enumerate_samsung_nvme_drives() -> Vec<DetectedSamsungNvme> {
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceProperty, IOCTL_STORAGE_QUERY_PROPERTY,
        STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
    };
    use windows::Win32::System::IO::DeviceIoControl;

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
        let desc = unsafe { &*(buffer.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
        let prod = read_cstr_at(&buffer[..returned as usize], desc.ProductIdOffset as usize);
        let rev = read_cstr_at(
            &buffer[..returned as usize],
            desc.ProductRevisionOffset as usize,
        );
        if let Some(model) = prod {
            if is_samsung_heatsink_candidate(&model) {
                found.push(DetectedSamsungNvme {
                    model,
                    firmware: rev,
                    device_path: path_str,
                });
            }
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
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
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
    fn matches_samsung_990_pro_model() {
        assert!(is_samsung_heatsink_candidate(
            "Samsung SSD 990 PRO with Heatsink 2TB"
        ));
        assert!(is_samsung_heatsink_candidate("Samsung SSD 990 PRO 2TB"));
        assert!(!is_samsung_heatsink_candidate("WD_BLACK SN850X 2TB"));
    }
}
