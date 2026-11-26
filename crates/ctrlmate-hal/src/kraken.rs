//! Native USB HID driver for NZXT Kraken AIO coolers (`1E71:*`): X3, Z3, 2023 / Elite / Plus.
//!
//! Protocol (verified against liquidctl `liquidctl/driver/kraken3.py` and the Linux
//! `nzxt-kraken3` hwmon driver):
//!
//! * Reports are 64 bytes and the first byte is the opcode / report id; liquidctl writes exactly 64
//!   bytes (`data + zero padding`). Some hidapi back-ends want a leading `0x00` report-id byte
//!   instead (65 bytes), so [`HidIo`] tries 64 bytes first and falls back to 65 once, then remembers
//!   what worked.
//! * Init: `70 02 01 B8 <interval>` then `70 01` (start status reports). Z3 / 2023 additionally need
//!   `74 01` before each status read; X3 streams them.
//! * Status `75 01 ...`: liquid `msg[15] + msg[16]/10`, pump rpm `msg[17..19]` LE, pump duty `msg[19]`,
//!   fan rpm `msg[23..25]` LE, fan duty `msg[25]` (Z3 / 2023 only). `FF FF` in the temperature bytes
//!   is a firmware fault.
//! * Duty curve: opcode `72` + 3 channel bytes + 40 duties for liquid temperatures 20..=59 C.
//!   Channel bytes: pump `01 00 00` (X3, Z3) / `01 01 00` (2023); fan `02 00 00` (Z3) / `02 01 01` (2023).
//! * No usage-page filter: liquidctl matches by VID/PID only and the Kraken exposes a single HID
//!   interface (the LCD uses bulk endpoints). A vendor page `0xFF00` interface is preferred when
//!   several exist.
//!
//! Safety: every curve written here forces 100 % at and above the configured critical liquid
//! temperature, so a daemon crash leaves the cooler with a firmware-side emergency point. The pump
//! never gets less than 50 %. A read failure is an error, never a made-up reading.

use anyhow::{Context, Result, bail};
use ctrlmate_protocol::MIN_PUMP_PERCENT;
use hidapi::{HidApi, HidDevice};
use std::sync::atomic::{AtomicU8, Ordering};
use tracing::{debug, error, info, warn};

pub const NZXT_VID: u16 = 0x1E71;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Kraken X53 / X63 / X73: pump only.
    X3,
    /// Kraken Z53 / Z63 / Z73.
    Z3,
    /// Kraken 2023, Elite, Elite RGB, Plus.
    Gen2023,
}

/// `(PID, model, family, has_fan_channel)`.
pub const KRAKEN_PIDS: &[(u16, &str, Family, bool)] = &[
    (0x2007, "NZXT Kraken X3 (X53/X63/X73)", Family::X3, false),
    (0x2014, "NZXT Kraken X3 (X53/X63/X73)", Family::X3, false),
    (0x3008, "NZXT Kraken Z3 (Z53/Z63/Z73)", Family::Z3, true),
    (0x300C, "NZXT Kraken Elite (2023)", Family::Gen2023, true),
    (
        0x300E,
        "NZXT Kraken 2023 (240/280/360)",
        Family::Gen2023,
        true,
    ),
    (
        0x3012,
        "NZXT Kraken Elite RGB (2024)",
        Family::Gen2023,
        true,
    ),
    (0x3014, "NZXT Kraken Plus (2024)", Family::Gen2023, true),
];

const PACKET_LEN: usize = 64;
const STATUS_REPORT_ID: u8 = 0x75;
/// Liquid temperatures covered by a duty curve: 20..=59 C.
pub const CURVE_POINTS: usize = 40;
pub const CURVE_FIRST_C: u8 = 20;
pub const CURVE_LAST_C: u8 = 59;
/// Lowest radiator-fan duty this driver writes (a stalled radiator is never intended).
pub const MIN_FAN_PERCENT: u8 = 25;
const MAX_READ_ATTEMPTS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Pump,
    Fan,
}

fn channel_bytes(family: Family, ch: Channel) -> [u8; 3] {
    match (family, ch) {
        (Family::Gen2023, Channel::Pump) => [0x01, 0x01, 0x00],
        (Family::Gen2023, Channel::Fan) => [0x02, 0x01, 0x01],
        (_, Channel::Pump) => [0x01, 0x00, 0x00],
        (_, Channel::Fan) => [0x02, 0x00, 0x00],
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KrakenStatus {
    pub liquid_temp_c: Option<f64>,
    pub pump_rpm: Option<u16>,
    pub pump_duty_pct: Option<u8>,
    pub fan_rpm: Option<u16>,
    pub fan_duty_pct: Option<u8>,
}

/// Byte transport; implemented for real HID and for tests.
pub trait KrakenIo: Send {
    fn write(&self, packet: &[u8; PACKET_LEN]) -> Result<()>;
    /// Reads one report into `buf`, waiting at most `timeout_ms`; `Ok(0)` is a timeout.
    fn read(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize>;
}

/// hidapi transport with the 64 / 65 byte framing fallback.
pub struct HidIo {
    device: HidDevice,
    /// 0 = untested, 1 = 64 bytes work, 2 = 65 bytes (leading report id 0) work.
    framing: AtomicU8,
}

impl KrakenIo for HidIo {
    fn write(&self, pkt: &[u8; PACKET_LEN]) -> Result<()> {
        let mode = self.framing.load(Ordering::Relaxed);
        if mode != 2 {
            match self.device.write(pkt) {
                Ok(_) => {
                    self.framing.store(1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(e64) if mode == 1 => bail!("HID write failed: {e64}"),
                Err(e64) => debug!("64-byte HID write failed ({e64}); trying 65-byte framing"),
            }
        }
        let mut p65 = [0u8; PACKET_LEN + 1];
        p65[1..].copy_from_slice(pkt);
        self.device
            .write(&p65)
            .map(|_| self.framing.store(2, Ordering::Relaxed))
            .context("HID write failed with both 64- and 65-byte framing")
    }

    fn read(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize> {
        self.device
            .read_timeout(buf, timeout_ms)
            .context("Kraken HID read failed")
    }
}

/// Outcome of one apply call. `None` = nothing needed writing for that channel.
#[derive(Debug, Default)]
pub struct ApplyReport {
    pub pump: Option<Result<()>>,
    pub fan: Option<Result<()>>,
}

impl ApplyReport {
    pub fn all_ok(&self) -> bool {
        self.pump.as_ref().is_none_or(Result::is_ok) && self.fan.as_ref().is_none_or(Result::is_ok)
    }
}

pub struct KrakenDriver {
    pid: u16,
    model_name: String,
    family: Family,
    has_fan_channel: bool,
    io: Box<dyn KrakenIo>,
    /// Last duty that was *successfully* written (never set on failure, so a failed write is retried).
    applied_pump: Option<u8>,
    applied_fan: Option<u8>,
    defaults_restored: bool,
}

impl KrakenDriver {
    /// Finds and opens the first Kraken. Returns `None` when there is none or it cannot be opened
    /// (typically NZXT CAM holds it; the reason is logged).
    pub fn probe() -> Option<Self> {
        let api = HidApi::new().ok()?;
        let mut candidates: Vec<_> = api
            .device_list()
            .filter(|d| d.vendor_id() == NZXT_VID)
            .filter_map(|d| {
                KRAKEN_PIDS
                    .iter()
                    .find(|k| k.0 == d.product_id())
                    .map(|k| (d, *k))
            })
            .collect();
        // Prefer the vendor-defined usage page.
        candidates.sort_by_key(|(d, _)| u8::from(d.usage_page() != 0xFF00));
        for (info, (pid, label, family, has_fan)) in candidates {
            match api.open_path(info.path()) {
                Ok(device) => {
                    let io = HidIo {
                        device,
                        framing: AtomicU8::new(0),
                    };
                    match Self::init(Box::new(io), pid, label, family, has_fan) {
                        Ok(d) => {
                            info!("Initialized NZXT Kraken driver: {label} (1E71:{pid:04X})");
                            return Some(d);
                        }
                        Err(e) => warn!("Kraken {label} (1E71:{pid:04X}) init failed: {e:#}"),
                    }
                }
                Err(err) => warn!(
                    "Found {label} (1E71:{pid:04X}) but opening it failed ({err}); close NZXT CAM if it is running"
                ),
            }
        }
        None
    }

    /// Sends the init sequence over `io` and builds the driver (also used by tests).
    pub fn init(
        io: Box<dyn KrakenIo>,
        pid: u16,
        label: &str,
        family: Family,
        has_fan_channel: bool,
    ) -> Result<Self> {
        let mut p = [0u8; PACKET_LEN];
        p[..5].copy_from_slice(&[0x70, 0x02, 0x01, 0xB8, 0x01]);
        io.write(&p).context("Kraken init (update interval)")?;
        let mut p = [0u8; PACKET_LEN];
        p[..2].copy_from_slice(&[0x70, 0x01]);
        io.write(&p).context("Kraken init (start status)")?;
        Ok(Self {
            pid,
            model_name: label.into(),
            family,
            has_fan_channel,
            io,
            applied_pump: None,
            applied_fan: None,
            defaults_restored: false,
        })
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

    /// Last duties successfully written `(pump, fan)`.
    pub fn applied(&self) -> (Option<u8>, Option<u8>) {
        (self.applied_pump, self.applied_fan)
    }

    /// Forgets what was written (hardware re-probed, bridge restarted): the next apply rewrites.
    pub fn invalidate(&mut self) {
        self.applied_pump = None;
        self.applied_fan = None;
    }

    /// Reads one status report. An error means *no reading* (never a default).
    pub fn poll_status(&self) -> Result<KrakenStatus> {
        if self.family != Family::X3 {
            let mut req = [0u8; PACKET_LEN];
            req[..2].copy_from_slice(&[0x74, 0x01]);
            self.io.write(&req).context("Kraken status request")?;
        }
        let mut buf = [0u8; PACKET_LEN + 1];
        for _ in 0..MAX_READ_ATTEMPTS {
            let n = self.io.read(&mut buf, 100)?;
            if n == 0 {
                continue;
            }
            if let Some(st) = parse_status_report(&buf[..n], self.has_fan_channel) {
                return Ok(st);
            }
        }
        bail!("no Kraken status report within {MAX_READ_ATTEMPTS} reads")
    }

    fn write_curve(&mut self, ch: Channel, packet: &[u8; PACKET_LEN]) -> Result<()> {
        self.io
            .write(packet)
            .with_context(|| format!("Kraken {ch:?} curve write"))
    }

    /// Applies the targets. Pump and fan are written independently: a failing pump write never
    /// skips the fan write and vice versa. Only a successful write marks a target as applied.
    /// Every curve ends in 100 % from `critical_c` upwards.
    pub fn apply(
        &mut self,
        pump_percent: u8,
        fan_percent: Option<u8>,
        critical_c: f64,
    ) -> ApplyReport {
        let mut rep = ApplyReport::default();
        let pump = pump_percent.clamp(MIN_PUMP_PERCENT, 100);
        if self.applied_pump != Some(pump) {
            let pkt = build_duty_curve_packet(
                channel_bytes(self.family, Channel::Pump),
                pump,
                MIN_PUMP_PERCENT,
                critical_c,
            );
            let r = self.write_curve(Channel::Pump, &pkt);
            match &r {
                Ok(()) => {
                    self.applied_pump = Some(pump);
                    debug!("Kraken pump duty applied: {pump}%");
                }
                Err(e) => error!("Kraken pump write failed (will retry): {e:#}"),
            }
            rep.pump = Some(r);
        }
        if self.has_fan_channel
            && let Some(fan) = fan_percent
        {
            let fan = fan.clamp(MIN_FAN_PERCENT, 100);
            if self.applied_fan != Some(fan) {
                let pkt = build_duty_curve_packet(
                    channel_bytes(self.family, Channel::Fan),
                    fan,
                    MIN_FAN_PERCENT,
                    critical_c,
                );
                let r = self.write_curve(Channel::Fan, &pkt);
                match &r {
                    Ok(()) => {
                        self.applied_fan = Some(fan);
                        debug!("Kraken fan duty applied: {fan}%");
                    }
                    Err(e) => error!("Kraken fan write failed (will retry): {e:#}"),
                }
                rep.fan = Some(r);
            }
        }
        rep
    }

    /// Writes the built-in default (firmware-style) curves for pump and fan. Called on drop.
    pub fn restore_defaults(&mut self) -> Result<()> {
        let pump = self.write_curve(
            Channel::Pump,
            &build_default_curve_packet(channel_bytes(self.family, Channel::Pump), true),
        );
        let fan = if self.has_fan_channel {
            self.write_curve(
                Channel::Fan,
                &build_default_curve_packet(channel_bytes(self.family, Channel::Fan), false),
            )
        } else {
            Ok(())
        };
        self.invalidate();
        match (pump, fan) {
            (Ok(()), Ok(())) => {
                self.defaults_restored = true;
                info!("Kraken default curves restored");
                Ok(())
            }
            (a, b) => {
                let e = a.err().or(b.err());
                bail!("Kraken default curve restore failed: {e:#?}")
            }
        }
    }
}

impl Drop for KrakenDriver {
    fn drop(&mut self) {
        if !self.defaults_restored
            && let Err(e) = self.restore_defaults()
        {
            error!("{e:#}");
        }
    }
}

/// Parses a `75 01` status report (with or without a leading report-id `00`). `None` if it is not one.
pub fn parse_status_report(buf: &[u8], has_fan_channel: bool) -> Option<KrakenStatus> {
    let b = if buf.len() >= 27 && buf[0] == 0x00 && buf[1] == STATUS_REPORT_ID {
        &buf[1..]
    } else {
        buf
    };
    if b.len() < 20 || b[0] != STATUS_REPORT_ID || b[1] != 0x01 {
        return None;
    }
    let (whole, tenths) = (b[15], b[16]);
    let liquid_temp_c = if (whole, tenths) == (0xFF, 0xFF) {
        warn!("Kraken reports a firmware fault (temperature bytes FF FF)");
        None
    } else if (5..=90).contains(&whole) && tenths < 10 {
        Some(f64::from(whole) + f64::from(tenths) / 10.0)
    } else {
        None
    };
    let (fan_rpm, fan_duty_pct) = if has_fan_channel && b.len() >= 26 {
        (
            Some(u16::from_le_bytes([b[23], b[24]])),
            Some(b[25].min(100)),
        )
    } else {
        (None, None)
    };
    Some(KrakenStatus {
        liquid_temp_c,
        pump_rpm: Some(u16::from_le_bytes([b[17], b[18]])),
        pump_duty_pct: Some(b[19].min(100)),
        fan_rpm,
        fan_duty_pct,
    })
}

/// `72 <3 channel bytes>` + 40 duties (liquid 20..=59 C). Duties are `duty` (clamped to
/// `min_duty..=100`) below `critical_c` and 100 from `critical_c` up: the firmware-side emergency
/// point. The last point (59 C) is always 100.
pub fn build_duty_curve_packet(
    channel: [u8; 3],
    duty: u8,
    min_duty: u8,
    critical_c: f64,
) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x72;
    pkt[1..4].copy_from_slice(&channel);
    let target = duty.clamp(min_duty, 100);
    let crit = if critical_c.is_finite() {
        critical_c.clamp(f64::from(CURVE_FIRST_C), f64::from(CURVE_LAST_C))
    } else {
        f64::from(CURVE_LAST_C)
    };
    for i in 0..CURVE_POINTS {
        let temp_c = f64::from(CURVE_FIRST_C) + i as f64;
        pkt[4 + i] = if temp_c >= crit { 100 } else { target };
    }
    pkt[4 + CURVE_POINTS - 1] = 100;
    pkt
}

/// Built-in default curve (what is written on shutdown): quiet when cool, 100 % from 45 C.
pub fn build_default_curve_packet(channel: [u8; 3], is_pump: bool) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x72;
    pkt[1..4].copy_from_slice(&channel);
    for i in 0..CURVE_POINTS {
        let temp_c = CURVE_FIRST_C as usize + i;
        pkt[4 + i] = match (is_pump, temp_c) {
            (true, 0..=29) => 60,
            (true, 30..=39) => 75,
            (true, 40..=44) => 90,
            (false, 0..=28) => 30,
            (false, 29..=34) => 40,
            (false, 35..=39) => 55,
            (false, 40..=44) => 75,
            (false, 45..=49) => 90,
            _ => 100,
        };
    }
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Log {
        writes: Vec<[u8; PACKET_LEN]>,
        fail_opcode_channel: Vec<u8>,
        reads: Vec<Vec<u8>>,
        fail_all_writes: bool,
    }

    #[derive(Clone, Default)]
    struct MockIo(Arc<Mutex<Log>>);

    impl KrakenIo for MockIo {
        fn write(&self, pkt: &[u8; PACKET_LEN]) -> Result<()> {
            let mut l = self.0.lock().unwrap();
            if l.fail_all_writes || (pkt[0] == 0x72 && l.fail_opcode_channel.contains(&pkt[1])) {
                bail!("mock write failure");
            }
            l.writes.push(*pkt);
            Ok(())
        }
        fn read(&self, buf: &mut [u8], _t: i32) -> Result<usize> {
            let mut l = self.0.lock().unwrap();
            if l.reads.is_empty() {
                return Ok(0);
            }
            let r = l.reads.remove(0);
            buf[..r.len()].copy_from_slice(&r);
            Ok(r.len())
        }
    }

    fn driver(family: Family, fan: bool) -> (KrakenDriver, MockIo) {
        let io = MockIo::default();
        let d = KrakenDriver::init(Box::new(io.clone()), 0x300E, "t", family, fan).unwrap();
        (d, io)
    }

    fn status_report(temp: (u8, u8)) -> Vec<u8> {
        let mut b = vec![0u8; 64];
        b[0] = 0x75;
        b[1] = 0x01;
        b[15] = temp.0;
        b[16] = temp.1;
        b[17..19].copy_from_slice(&2450u16.to_le_bytes());
        b[19] = 75;
        b[23..25].copy_from_slice(&1120u16.to_le_bytes());
        b[25] = 45;
        b
    }

    #[test]
    fn init_sends_both_liquidctl_packets() {
        let (_d, io) = driver(Family::Z3, true);
        let w = io.0.lock().unwrap();
        assert_eq!(&w.writes[0][..5], &[0x70, 0x02, 0x01, 0xB8, 0x01]);
        assert_eq!(&w.writes[1][..2], &[0x70, 0x01]);
    }

    #[test]
    fn parses_status_report_accurately() {
        let st = parse_status_report(&status_report((31, 4)), true).unwrap();
        assert_eq!(st.liquid_temp_c, Some(31.4));
        assert_eq!(st.pump_rpm, Some(2450));
        assert_eq!(st.pump_duty_pct, Some(75));
        assert_eq!(st.fan_rpm, Some(1120));
        assert_eq!(st.fan_duty_pct, Some(45));
        // X3 has no fan channel: no fan values are invented.
        let st = parse_status_report(&status_report((31, 4)), false).unwrap();
        assert_eq!((st.fan_rpm, st.fan_duty_pct), (None, None));
        // Leading report-id byte variant.
        let mut shifted = vec![0u8];
        shifted.extend(status_report((40, 0)));
        assert_eq!(
            parse_status_report(&shifted, true).unwrap().liquid_temp_c,
            Some(40.0)
        );
    }

    #[test]
    fn faults_and_garbage_are_not_readings() {
        let st = parse_status_report(&status_report((0xFF, 0xFF)), true).unwrap();
        assert_eq!(st.liquid_temp_c, None);
        assert!(parse_status_report(&[0x75, 0x01, 0, 0], true).is_none());
        assert!(parse_status_report(&[0x11; 64], true).is_none());
        let st = parse_status_report(&status_report((3, 0)), true).unwrap();
        assert_eq!(st.liquid_temp_c, None, "3 C coolant is implausible");
    }

    #[test]
    fn read_failure_is_an_error_not_defaults() {
        let (d, io) = driver(Family::Z3, true);
        // No report ever arrives.
        assert!(d.poll_status().is_err());
        io.0.lock().unwrap().reads.push(vec![0x11; 64]);
        assert!(d.poll_status().is_err());
        io.0.lock().unwrap().reads.push(status_report((30, 0)));
        assert_eq!(d.poll_status().unwrap().liquid_temp_c, Some(30.0));
    }

    #[test]
    fn x3_does_not_send_a_status_request() {
        let (d, io) = driver(Family::X3, false);
        let before = io.0.lock().unwrap().writes.len();
        io.0.lock().unwrap().reads.push(status_report((30, 0)));
        d.poll_status().unwrap();
        assert_eq!(io.0.lock().unwrap().writes.len(), before);
    }

    #[test]
    fn curve_has_firmware_100_at_critical_and_pump_floor() {
        let pkt = build_duty_curve_packet([1, 0, 0], 20, MIN_PUMP_PERCENT, 55.0);
        assert_eq!(&pkt[..4], &[0x72, 1, 0, 0]);
        assert_eq!(pkt[4], 50, "20 C: pump floored to 50");
        assert_eq!(pkt[4 + 34], 50, "54 C below critical");
        assert_eq!(pkt[4 + 35], 100, "55 C == critical");
        assert_eq!(pkt[4 + 39], 100, "59 C always 100");
        // A critical beyond the table still ends in 100.
        let pkt = build_duty_curve_packet([2, 0, 0], 30, MIN_FAN_PERCENT, 120.0);
        assert_eq!(pkt[4 + 38], 30);
        assert_eq!(pkt[4 + 39], 100);
        let pkt = build_duty_curve_packet([2, 0, 0], 30, MIN_FAN_PERCENT, f64::NAN);
        assert_eq!(pkt[4 + 39], 100);
    }

    #[test]
    fn channel_ids_follow_liquidctl() {
        assert_eq!(channel_bytes(Family::Z3, Channel::Fan), [2, 0, 0]);
        assert_eq!(channel_bytes(Family::X3, Channel::Pump), [1, 0, 0]);
        assert_eq!(channel_bytes(Family::Gen2023, Channel::Pump), [1, 1, 0]);
        assert_eq!(channel_bytes(Family::Gen2023, Channel::Fan), [2, 1, 1]);
    }

    #[test]
    fn pump_failure_does_not_skip_the_fan_write_and_vice_versa() {
        let (mut d, io) = driver(Family::Z3, true);
        io.0.lock().unwrap().fail_opcode_channel = vec![0x01]; // pump channel
        let rep = d.apply(60, Some(40), 55.0);
        assert!(rep.pump.as_ref().unwrap().is_err());
        assert!(rep.fan.as_ref().unwrap().is_ok());
        assert_eq!(
            d.applied(),
            (None, Some(40)),
            "failed pump must not be marked applied"
        );
        // Retry next tick: pump written again, fan not rewritten.
        io.0.lock().unwrap().fail_opcode_channel.clear();
        let rep = d.apply(60, Some(40), 55.0);
        assert!(rep.pump.as_ref().unwrap().is_ok());
        assert!(rep.fan.is_none());
        assert_eq!(d.applied(), (Some(60), Some(40)));
        // Fan failure, pump success.
        io.0.lock().unwrap().fail_opcode_channel = vec![0x02];
        let rep = d.apply(70, Some(50), 55.0);
        assert!(rep.pump.as_ref().unwrap().is_ok());
        assert!(rep.fan.as_ref().unwrap().is_err());
        assert_eq!(d.applied(), (Some(70), Some(40)));
        assert!(!rep.all_ok());
    }

    #[test]
    fn pump_is_never_written_below_50() {
        let (mut d, io) = driver(Family::Z3, true);
        let rep = d.apply(0, Some(0), 55.0);
        assert!(rep.all_ok());
        assert_eq!(d.applied(), (Some(50), Some(MIN_FAN_PERCENT)));
        let l = io.0.lock().unwrap();
        let pump_pkt = l.writes.iter().find(|p| p[0] == 0x72 && p[1] == 1).unwrap();
        assert!(pump_pkt[4..4 + CURVE_POINTS].iter().all(|&d| d >= 50));
    }

    #[test]
    fn x3_never_writes_a_fan_channel() {
        let (mut d, io) = driver(Family::X3, false);
        let rep = d.apply(60, Some(80), 55.0);
        assert!(rep.fan.is_none());
        assert!(
            io.0.lock()
                .unwrap()
                .writes
                .iter()
                .all(|p| !(p[0] == 0x72 && p[1] == 2))
        );
    }

    #[test]
    fn drop_restores_the_default_curves() {
        let (mut d, io) = driver(Family::Z3, true);
        d.apply(60, Some(40), 55.0);
        let before = io.0.lock().unwrap().writes.len();
        drop(d);
        let l = io.0.lock().unwrap();
        let after: Vec<_> = l.writes[before..].to_vec();
        assert_eq!(after.len(), 2, "pump + fan default curves");
        assert!(after.iter().all(|p| p[0] == 0x72 && p[4 + 39] == 100));
        assert!(after[0][4] >= 50, "default pump curve respects the floor");
    }

    #[test]
    fn restore_failure_is_reported() {
        let (mut d, io) = driver(Family::Z3, true);
        io.0.lock().unwrap().fail_all_writes = true;
        assert!(d.restore_defaults().is_err());
        io.0.lock().unwrap().fail_all_writes = false;
        assert!(d.restore_defaults().is_ok());
    }
}
