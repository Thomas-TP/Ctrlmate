//! Native USB HID driver for Gigabyte RGB Fusion 2.0 motherboard controllers (ITE IT5702 / IT8297).
//!
//! Zero kernel drivers required — works identically on Windows (HID Feature Reports) and Linux (`hidraw`).

use anyhow::{Context, Result};
use ctrlmate_protocol::RgbColor;
use hidapi::{HidApi, HidDevice};
use tracing::{debug, info, warn};

pub const GIGABYTE_ITE_VID: u16 = 0x048D;
pub const GIGABYTE_ITE_PIDS: &[u16] = &[0x5702, 0x8297, 0x5711];

const REPORT_ID: u8 = 0xCC;
const PACKET_LEN: usize = 64;
const EFFECT_STATIC: u8 = 0x01;
const ZONE_COUNT: u8 = 8; // Headers 0x20..=0x27 (onboard + D_LED1/2 + LED_C1/2)

pub struct GigabyteIteBackend {
    pid: u16,
    device: Option<HidDevice>,
    firmware_name: String,
    last_color: Option<RgbColor>,
}

impl GigabyteIteBackend {
    pub fn probe(api: &HidApi) -> Option<Self> {
        for dev_info in api.device_list() {
            if dev_info.vendor_id() == GIGABYTE_ITE_VID
                && GIGABYTE_ITE_PIDS.contains(&dev_info.product_id())
            {
                // On Windows, IT5702 exposes Col01 (Usage 0x0010, 17B) and Col02 (Usage 0x00CC, 64B RGB).
                // On Linux hidraw, usage_page/usage may be 0, so accept either 0 or (0xFF89, 0x00CC).
                let up = dev_info.usage_page();
                let u = dev_info.usage();
                if up != 0 && !(up == 0xFF89 && u == 0x00CC) {
                    continue;
                }
                let pid = dev_info.product_id();
                match api.open_path(dev_info.path()) {
                    Ok(device) => {
                        let fw = Self::query_firmware(&device)
                            .unwrap_or_else(|_| format!("Gigabyte ITE 0x{pid:04X}"));
                        info!("Initialized Gigabyte RGB USB HID backend: {fw} (048D:{pid:04X})");
                        return Some(Self {
                            pid,
                            device: Some(device),
                            firmware_name: fw,
                            last_color: None,
                        });
                    }
                    Err(err) => {
                        warn!(
                            "Found Gigabyte ITE RGB controller 048D:{pid:04X} but failed to open HID handle ({err}) — close GIGABYTE Control Center if running"
                        );
                    }
                }
            }
        }
        None
    }

    pub fn name(&self) -> String {
        format!("{} (048D:{:04X})", self.firmware_name, self.pid)
    }

    fn query_firmware(dev: &HidDevice) -> Result<String> {
        let mut init = [0u8; PACKET_LEN];
        init[0] = REPORT_ID;
        init[1] = 0x60;
        dev.send_feature_report(&init)
            .context("IT5702 init feature report 0xCC 0x60 failed")?;

        let mut resp = [0u8; PACKET_LEN];
        resp[0] = REPORT_ID;
        let len = dev.get_feature_report(&mut resp).unwrap_or(0);
        if len >= 20 {
            let ascii: String = resp[12..len.min(44)]
                .iter()
                .take_while(|&&b| b != 0)
                .filter(|&&b| b.is_ascii_graphic() || b == b' ')
                .map(|&b| b as char)
                .collect();
            let trimmed = ascii.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }
        Ok("Gigabyte ITE RGB Fusion".into())
    }

    /// Applies a uniform static RGB color across all Gigabyte motherboard & ARGB header zones.
    pub fn apply_color(&mut self, color: RgbColor) -> Result<()> {
        if self.last_color == Some(color) {
            return Ok(());
        }
        let Some(dev) = self.device.as_ref() else {
            return Ok(());
        };

        // Iterate zones 0..8 (IT5702 ignores single broadcast packets on newer firmwares).
        for zone in 0..ZONE_COUNT {
            let pkt = build_effect_packet(zone, color);
            dev.send_feature_report(&pkt)
                .with_context(|| format!("IT5702 SendEffect failed on zone {zone}"))?;
        }

        let apply = build_apply_packet();
        dev.send_feature_report(&apply)
            .context("IT5702 ApplyEffect (0xCC 0x28 0xFF) failed")?;

        self.last_color = Some(color);
        debug!(
            "Gigabyte IT5702 applied RGB({}, {}, {})",
            color.r, color.g, color.b
        );
        Ok(())
    }
}

/// Constructs an OpenRGB-compatible 64-byte `PktEffect` feature report for IT5702/IT8297.
pub fn build_effect_packet(zone_index: u8, color: RgbColor) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = REPORT_ID;
    pkt[1] = 0x20 + zone_index;

    let zone_mask: u32 = 1u32 << zone_index;
    pkt[2..6].copy_from_slice(&zone_mask.to_le_bytes());
    // zone1 [6..10] = 0, reserved0 [10] = 0

    let is_off = color.r == 0 && color.g == 0 && color.b == 0;
    pkt[11] = EFFECT_STATIC;
    pkt[12] = if is_off { 0x00 } else { 0x64 }; // max_brightness (0..=100)
    pkt[13] = 0x00; // min_brightness

    // color0 in little-endian 0x00RRGGBB -> bytes [B, G, R, 0x00]
    pkt[14] = color.b;
    pkt[15] = color.g;
    pkt[16] = color.r;
    pkt[17] = 0x00;

    pkt[32] = 0x01; // effect_param2 = 1 color
    pkt
}

pub fn build_apply_packet() -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = REPORT_ID;
    pkt[1] = 0x28;
    pkt[2] = 0xFF;
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it5702_effect_packet_layout_matches_spec() {
        let pkt = build_effect_packet(
            2,
            RgbColor {
                r: 0xAA,
                g: 0xBB,
                b: 0xCC,
            },
        );
        assert_eq!(pkt[0], 0xCC);
        assert_eq!(pkt[1], 0x22);
        assert_eq!(u32::from_le_bytes([pkt[2], pkt[3], pkt[4], pkt[5]]), 4);
        assert_eq!(pkt[11], 0x01);
        assert_eq!(pkt[12], 0x64);
        assert_eq!([pkt[14], pkt[15], pkt[16], pkt[17]], [0xCC, 0xBB, 0xAA, 0x00]);
    }
}
