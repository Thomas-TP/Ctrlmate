//! Native USB HID driver for NZXT RGB Controllers (`1E71:2012`, Smart Device V2, RGB & Fan Controller).
//!
//! Zero kernel drivers required — works identically on Windows and Linux (`hidraw`).

use anyhow::{Context, Result};
use ctrlmate_protocol::RgbColor;
use hidapi::{HidApi, HidDevice};
use tracing::{debug, info, warn};

pub const NZXT_VID: u16 = 0x1E71;

/// Supported NZXT standalone RGB & Smart Device V2 product IDs.
pub const NZXT_RGB_PIDS: &[(u16, u8, &str)] = &[
    (0x2012, 3, "NZXT RGB Controller (3-ch)"),
    (0x2011, 6, "NZXT RGB & Fan Controller (6-ch)"),
    (0x2019, 6, "NZXT RGB & Fan Controller v2 (6-ch)"),
    (0x2020, 6, "NZXT RGB & Fan Controller (2024)"),
    (0x2006, 2, "NZXT Smart Device V2"),
    (0x2009, 2, "NZXT RGB & Fan Controller"),
];

const PACKET_LEN: usize = 64;
/// Standard NZXT HUE2 / SmartDevice2 max LEDs per channel (up to 40 LEDs per channel).
const MAX_LEDS_PER_CHANNEL: usize = 40;

pub struct NzxtRgbBackend {
    pid: u16,
    path: String,
    channels: u8,
    label: String,
    device: Option<HidDevice>,
    last_color: Option<RgbColor>,
}

impl NzxtRgbBackend {
    pub fn probe(api: &HidApi) -> Vec<Self> {
        let mut found = Vec::new();
        for dev_info in api.device_list() {
            if dev_info.vendor_id() != NZXT_VID {
                continue;
            }
            let pid = dev_info.product_id();
            let Some(&(_, channels, label)) = NZXT_RGB_PIDS.iter().find(|&&(p, _, _)| p == pid)
            else {
                continue;
            };

            let path = dev_info.path().to_string_lossy().to_string();
            // Deduplicate by unique device path (a PC like NZXT H9 Flow can have 2-3 controllers with the same PID!)
            if found.iter().any(|b: &Self| b.path == path) {
                continue;
            }

            match api.open_path(dev_info.path()) {
                Ok(device) => {
                    let idx = found.len() + 1;
                    info!("Initialized {label} #{idx} USB HID backend (1E71:{pid:04X}, {channels} channels)");
                    found.push(Self {
                        pid,
                        path,
                        channels,
                        label: format!("{label} #{idx}"),
                        device: Some(device),
                        last_color: None,
                    });
                }
                Err(err) => {
                    warn!(
                        "Found {label} (1E71:{pid:04X}) but failed to open HID handle ({err}) — close NZXT CAM if running"
                    );
                }
            }
        }
        found
    }

    pub fn name(&self) -> String {
        format!("{} (1E71:{:04X})", self.label, self.pid)
    }

    fn write_report(dev: &HidDevice, pkt: &[u8; PACKET_LEN]) -> Result<()> {
        if let Err(err64) = dev.write(pkt) {
            let mut pkt65 = [0u8; PACKET_LEN + 1];
            pkt65[1..].copy_from_slice(pkt);
            dev.write(&pkt65)
                .with_context(|| format!("NZXT RGB HID write failed (64B: {err64})"))?;
        }
        Ok(())
    }

    /// Applies a static solid RGB color across all channels of the NZXT RGB controller.
    ///
    /// Uses NZXT HUE2 / SmartDevice2 direct GRB LED frame packets (`0x22 0x10` / `0x22 0x11`
    /// + `0x22 0xA0` apply) and `0x2A 0x04` static mode header for 2023+ `0x2012` controllers.
    pub fn apply_color(&mut self, color: RgbColor) -> Result<()> {
        if self.last_color == Some(color) {
            return Ok(());
        }
        let Some(dev) = self.device.as_ref() else {
            return Ok(());
        };

        for ch in 0..self.channels {
            if self.pid == 0x2012 {
                // 0x2012 supports 0x2A 0x04 fixed-color packets per channel (1-indexed)
                // as well as direct GRB buffer writes (0x22 0x10/0x11 + 0x22 0xA0).
                let pkt = build_2012_static_packet(ch + 1, color);
                Self::write_report(dev, &pkt)
                    .with_context(|| format!("NZXT 0x2012 static write failed on channel {}", ch + 1))?;
            }

            // Direct per-LED GRB chunks (works across HUE2, SmartDevice2, 0x2011, 0x2012, 0x2019)
            let channel_mask = 1u8 << ch;
            for chunk in build_direct_grb_packets(channel_mask, color, MAX_LEDS_PER_CHANNEL) {
                let _ = Self::write_report(dev, &chunk);
            }
            let apply = build_direct_apply_packet(channel_mask);
            let _ = Self::write_report(dev, &apply);
        }

        self.last_color = Some(color);
        debug!(
            "NZXT RGB (1E71:{:04X}) applied RGB({}, {}, {}) across {} channel(s)",
            self.pid, color.r, color.g, color.b, self.channels
        );
        Ok(())
    }
}

/// Builds the 64-byte `0x2A 0x04` fixed-color packet for NZXT `0x2012` controllers.
pub fn build_2012_static_packet(channel_1based: u8, color: RgbColor) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x2A;
    pkt[1] = 0x04;
    pkt[2] = channel_1based;
    pkt[3] = 0x01; // fixed/static mode
    pkt[4] = 0x00;
    pkt[5] = 0x64; // full scaling (brightness already baked into hardware RGB)
    pkt[6] = 0x00;
    // Fill RGB triplet (repeated across slots for multi-LED static frames)
    let mut idx = 7;
    while idx + 2 < PACKET_LEN {
        pkt[idx] = color.r;
        pkt[idx + 1] = color.g;
        pkt[idx + 2] = color.b;
        idx += 3;
    }
    pkt
}

/// Builds NZXT HUE2 / SmartDevice2 direct per-LED GRB packets (`0x22 0x10` & `0x22 0x11`).
pub fn build_direct_grb_packets(
    channel_mask: u8,
    color: RgbColor,
    led_count: usize,
) -> Vec<[u8; PACKET_LEN]> {
    let mut packets = Vec::new();
    let leds_per_pkt = 20; // 20 LEDs * 3 bytes = 60 bytes + 4-byte header = 64 bytes
    let total_chunks = led_count.div_ceil(leds_per_pkt);

    for chunk_idx in 0..total_chunks {
        let mut pkt = [0u8; PACKET_LEN];
        pkt[0] = 0x22;
        pkt[1] = 0x10 + (chunk_idx as u8);
        pkt[2] = channel_mask;
        pkt[3] = 0x00;

        let count_in_chunk = leds_per_pkt.min(led_count - chunk_idx * leds_per_pkt);
        for i in 0..count_in_chunk {
            let offset = 4 + i * 3;
            // NZXT HUE2 / SmartDevice2 direct LED buffers use GRB byte order
            pkt[offset] = color.g;
            pkt[offset + 1] = color.r;
            pkt[offset + 2] = color.b;
        }
        packets.push(pkt);
    }

    packets
}

pub fn build_direct_apply_packet(channel_mask: u8) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x22;
    pkt[1] = 0xA0;
    pkt[2] = channel_mask;
    pkt[3] = 0x00;
    pkt[4] = 0x01;
    pkt[5] = 0x00;
    pkt[6] = 0x00;
    pkt[7] = 0x28; // 40 LEDs
    pkt[8] = 0x00;
    pkt[9] = 0x00;
    pkt[10] = 0x80;
    pkt[11] = 0x00;
    pkt[12] = 0x32;
    pkt[13] = 0x00;
    pkt[14] = 0x00;
    pkt[15] = 0x01;
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nzxt_2012_and_direct_packets_are_64_bytes() {
        let color = RgbColor {
            r: 255,
            g: 160,
            b: 80,
        };
        let p = build_2012_static_packet(1, color);
        assert_eq!(p.len(), 64);
        assert_eq!(&p[0..4], &[0x2A, 0x04, 0x01, 0x01]);
        assert_eq!(&p[7..10], &[255, 160, 80]);

        let chunks = build_direct_grb_packets(0x01, color, 40);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0][0..3], [0x22, 0x10, 0x01]);
        assert_eq!(chunks[0][4..7], [160, 255, 80]); // GRB order
    }
}
