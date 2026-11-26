//! Cross-platform USB HID controller discovery (Zero-Kernel, identical on Windows & Linux).

use ctrlmate_protocol::DetectedHidDevice;
use hidapi::HidApi;
use std::collections::BTreeSet;
use tracing::{debug, warn};

/// Known USB Vendor IDs for PC RGB, motherboard ARGB headers & AIO liquid cooling controllers.
fn driver_hint_for_vid(vid: u16, product: &str) -> Option<&'static str> {
    match vid {
        0x1E71 => Some("nzxt_kraken_hid"),
        0x048D => {
            // ITE Tech (Gigabyte IT5701 / IT5702 RGB Fusion motherboard controller)
            Some("gigabyte_ite_rgb_hid")
        }
        0x0B05 => {
            let lower = product.to_ascii_lowercase();
            if lower.contains("aura") || lower.contains("rog") || lower.contains("led") {
                Some("asus_aura_hid")
            } else {
                None
            }
        }
        0x1B1C => Some("corsair_hid"),
        0x0CF2 => Some("lianli_uni_hid"),
        0x1462 | 0x0DB0 => Some("msi_mystic_hid"),
        0x0C70 => Some("aquacomputer_hid"),
        _ => None,
    }
}

pub fn scan_known_hid_devices() -> Vec<DetectedHidDevice> {
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(err) => {
            warn!("Unable to initialize USB HID scanner: {err}");
            return Vec::new();
        }
    };

    // Identical devices (three NZXT RGB controllers, ...) have different paths and must all be listed;
    // the same path can appear twice in the OS list, so the path is the identity.
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();

    for dev in api.device_list() {
        let vid = dev.vendor_id();
        let pid = dev.product_id();
        let product = dev.product_string().unwrap_or_default().trim().to_string();
        let manufacturer = dev
            .manufacturer_string()
            .unwrap_or_default()
            .trim()
            .to_string();

        let Some(hint) = driver_hint_for_vid(vid, &product) else {
            continue;
        };

        let path = dev.path().to_string_lossy().to_string();
        if !seen.insert(path.clone()) {
            continue;
        }
        debug!(
            "Discovered HID controller {:04x}:{:04x} ({product}) -> {hint}",
            vid, pid
        );

        found.push(DetectedHidDevice {
            vendor_id: vid,
            product_id: pid,
            manufacturer: if manufacturer.is_empty() {
                format!("VID 0x{vid:04X}")
            } else {
                manufacturer
            },
            product: if product.is_empty() {
                format!("USB Controller 0x{pid:04X}")
            } else {
                product
            },
            driver_hint: hint.to_string(),
            path,
        });
    }

    found.sort_by(|a, b| {
        (a.vendor_id, a.product_id, &a.path).cmp(&(b.vendor_id, b.product_id, &b.path))
    });
    found
}
