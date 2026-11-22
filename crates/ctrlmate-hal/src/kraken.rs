//! Native USB HID driver for NZXT Kraken AIO liquid coolers (`1E71:300E` Kraken 2023/360, Z3, X3, Elite).
//!
//! Protocol verified against Linux mainline `drivers/hwmon/nzxt-kraken3.c` and `liquidctl`.
//! Zero kernel drivers required — 100% Anti-Cheat Safe on Windows & Linux.

use anyhow::{Context, Result};
use ctrlmate_cooling::CoolingTargets;
use hidapi::{HidApi, HidDevice};
use tracing::{debug, info, warn};

pub const NZXT_VID: u16 = 0x1E71;

/// Supported Kraken 3 / Z3 / 2023 / Elite USB Product IDs.
pub const KRAKEN_PIDS: &[(u16, &str, bool)] = &[
    // (PID, friendly model, has_fan_channel)
    (0x300E, "NZXT Kraken 2023 (240/280/360)", true),
    (0x300C, "NZXT Kraken Elite (2023)", true),
    (0x3012, "NZXT Kraken Elite (2024)", true),
    (0x3008, "NZXT Kraken Z3 (Z53/Z63/Z73)", true),
    (0x2007, "NZXT Kraken X3 (X53/X63/X73)", false),
];

const PACKET_LEN: usize = 64;
const STATUS_REPORT_ID: u8 = 0x75;
const DUTY_CURVE_POINTS: usize = 40; // 20°C..=59°C

#[derive(Debug, Clone, Default)]
pub struct KrakenStatus {
    pub liquid_temp_c: Option<f64>,
    pub pump_rpm: Option<u16>,
    pub pump_duty_pct: Option<u8>,
    pub fan_rpm: Option<u16>,
    pub fan_duty_pct: Option<u8>,
}

pub struct KrakenDriver {
    pid: u16,
    model_name: String,
    has_fan_channel: bool,
    device: HidDevice,
    last_pump_duty: Option<u8>,
    last_fan_duty: Option<Option<u8>>,
}

impl KrakenDriver {
    pub fn probe() -> Option<Self> {
        let api = HidApi::new().ok()?;
        for dev_info in api.device_list() {
            if dev_info.vendor_id() != NZXT_VID {
                continue;
            }
            let pid = dev_info.product_id();
            let Some(&(_, label, has_fan)) = KRAKEN_PIDS.iter().find(|&&(p, _, _)| p == pid) else {
                continue;
            };

            // Kraken 2023 exposes Usage Page 0xFF00 for status/cooling control.
            if dev_info.usage_page() != 0 && dev_info.usage_page() != 0xFF00 {
                continue;
            }

            match api.open_path(dev_info.path()) {
                Ok(device) => {
                    // Send initial status-interval init packet (0x70 0x02 0x01 0xB8 0x01)
                    let mut init = [0u8; PACKET_LEN];
                    init[0] = 0x70;
                    init[1] = 0x02;
                    init[2] = 0x01;
                    init[3] = 0xB8;
                    init[4] = 0x01;
                    let _ = device.write(&init);

                    info!("Initialized NZXT Kraken USB HID driver: {label} (1E71:{pid:04X})");
                    return Some(Self {
                        pid,
                        model_name: label.into(),
                        has_fan_channel: has_fan,
                        device,
                        last_pump_duty: None,
                        last_fan_duty: None,
                    });
                }
                Err(err) => {
                    warn!(
                        "Found {label} (1E71:{pid:04X}) but failed to open HID handle ({err}) — close NZXT CAM if running"
                    );
                }
            }
        }
        None
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn has_fan_channel(&self) -> bool {
        self.has_fan_channel
    }

    pub fn pid(&self) -> u16 {
        self.pid
    }

    /// Queries real-time liquid temperature, pump RPM/duty and radiator fan RPM/duty.
    pub fn poll_status(&self) -> Result<KrakenStatus> {
        let mut req = [0u8; PACKET_LEN];
        req[0] = 0x74;
        req[1] = 0x01;
        if let Err(err) = self.device.write(&req) {
            // Fallback: some Windows HID stacks expect leading 0x00 report ID if unnumbered
            let mut req65 = [0u8; PACKET_LEN + 1];
            req65[1] = 0x74;
            req65[2] = 0x01;
            self.device
                .write(&req65)
                .with_context(|| format!("Kraken status request (0x74 0x01) failed (64B: {err})"))?;
        }

        let mut buf = [0u8; PACKET_LEN + 1];
        for _ in 0..4 {
            let n = self
                .device
                .read_timeout(&mut buf, 120)
                .context("Kraken status read failed")?;
            if n >= 26 && buf[0] == STATUS_REPORT_ID && buf[1] == 0x01 {
                return Ok(parse_kraken_status(&buf[..n], self.has_fan_channel));
            }
            if n >= 27 && buf[0] == 0x00 && buf[1] == STATUS_REPORT_ID && buf[2] == 0x01 {
                return Ok(parse_kraken_status(&buf[1..n], self.has_fan_channel));
            }
        }
        Ok(KrakenStatus::default())
    }

    fn write_report(&self, pkt: &[u8; PACKET_LEN]) -> Result<()> {
        if let Err(err64) = self.device.write(pkt) {
            let mut pkt65 = [0u8; PACKET_LEN + 1];
            pkt65[1..].copy_from_slice(pkt);
            self.device
                .write(&pkt65)
                .with_context(|| format!("HID write failed (64B: {err64})"))?;
        }
        Ok(())
    }

    /// Applies Watchdog-validated pump & radiator fan targets to the Kraken hardware.
    pub fn apply_cooling(&mut self, targets: CoolingTargets) -> Result<()> {
        // Always enforce pump minimum floor >= 50%
        let safe_pump = targets.aio_pump_percent.clamp(50, 100);
        if self.last_pump_duty != Some(safe_pump) {
            let pkt = build_duty_curve_packet(0x01, safe_pump, true);
            self.write_report(&pkt)
                .context("Kraken pump duty write failed")?;
            self.last_pump_duty = Some(safe_pump);
            debug!("NZXT Kraken pump duty applied: {safe_pump}%");
        }

        if self.has_fan_channel && self.last_fan_duty != Some(targets.aio_fan_percent) {
            let pkt = match targets.aio_fan_percent {
                Some(fixed) => build_duty_curve_packet(0x02, fixed.clamp(25, 100), false),
                None => build_auto_liquid_fan_curve_packet(0x02),
            };
            self.write_report(&pkt)
                .context("Kraken radiator fan curve write failed")?;
            self.last_fan_duty = Some(targets.aio_fan_percent);
            debug!("NZXT Kraken fan target applied: {:?}", targets.aio_fan_percent);
        }

        Ok(())
    }
}

/// Parses a 64-byte Kraken 3 / Z3 / 2023 `0x75` status report.
pub fn parse_kraken_status(buf: &[u8], has_fan_channel: bool) -> KrakenStatus {
    if buf.len() < 26 || buf[0] != STATUS_REPORT_ID {
        return KrakenStatus::default();
    }

    let whole = buf[15];
    let tenths = buf[16];
    let liquid_temp_c = if (5..=90).contains(&whole) && tenths < 10 {
        Some(f64::from(whole) + f64::from(tenths) / 10.0)
    } else {
        None
    };

    let pump_rpm = u16::from_le_bytes([buf[17], buf[18]]);
    let pump_duty = buf[19];

    let (fan_rpm, fan_duty_pct) = if has_fan_channel {
        let rpm = u16::from_le_bytes([buf[23], buf[24]]);
        let duty = buf[25];
        (Some(rpm), Some(duty.min(100)))
    } else {
        (None, None)
    };

    KrakenStatus {
        liquid_temp_c,
        pump_rpm: Some(pump_rpm),
        pump_duty_pct: Some(pump_duty.min(100)),
        fan_rpm,
        fan_duty_pct,
    }
}

/// Builds a 64-byte `0x72` duty curve packet (40 points for 20°C..=59°C liquid temp).
/// Always locks the top firmware emergency points (≥55°C liquid) to 100% as a hardware fail-safe.
pub fn build_duty_curve_packet(channel: u8, duty: u8, is_pump: bool) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x72;
    pkt[1] = channel;
    pkt[2] = 0x00;
    pkt[3] = 0x00;

    let min_duty = if is_pump { 50 } else { 20 };
    let target = duty.clamp(min_duty, 100);

    for i in 0..DUTY_CURVE_POINTS {
        let temp_c = 20 + i;
        pkt[4 + i] = if temp_c >= 55 { 100 } else { target };
    }
    pkt
}

/// Builds a smooth quiet-to-performance liquid temperature fan curve for `Auto` mode.
pub fn build_auto_liquid_fan_curve_packet(channel: u8) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x72;
    pkt[1] = channel;
    for i in 0..DUTY_CURVE_POINTS {
        let temp_c = 20 + i;
        let pct = match temp_c {
            0..=28 => 30,
            29..=34 => 40,
            35..=39 => 55,
            40..=44 => 75,
            45..=49 => 90,
            _ => 100,
        };
        pkt[4 + i] = pct;
    }
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kraken_status_report_accurately() {
        let mut buf = [0u8; 64];
        buf[0] = 0x75;
        buf[1] = 0x01;
        buf[15] = 31;
        buf[16] = 4; // 31.4°C
        buf[17..19].copy_from_slice(&2450u16.to_le_bytes());
        buf[19] = 75;
        buf[23..25].copy_from_slice(&1120u16.to_le_bytes());
        buf[25] = 45;

        let st = parse_kraken_status(&buf, true);
        assert_eq!(st.liquid_temp_c, Some(31.4));
        assert_eq!(st.pump_rpm, Some(2450));
        assert_eq!(st.pump_duty_pct, Some(75));
        assert_eq!(st.fan_rpm, Some(1120));
        assert_eq!(st.fan_duty_pct, Some(45));
    }

    #[test]
    fn kraken_curve_enforces_hardware_failsafe_at_55c() {
        let pkt = build_duty_curve_packet(0x01, 20, true);
        assert_eq!(pkt[4], 50); // Pump floored to 50% at 20°C
        assert_eq!(pkt[4 + 35], 100); // 55°C liquid forced to 100%
    }
}
