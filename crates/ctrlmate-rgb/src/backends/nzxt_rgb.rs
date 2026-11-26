//! NZXT RGB controllers over USB HID (`1E71:2012` "RGB Controller" 2023 and the Smart Device V2 /
//! HUE 2 family), zero kernel drivers.
//!
//! # Protocol (verified against primary sources)
//!
//! * liquidctl `liquidctl/driver/smart_device.py`: `SmartDevice2` (PIDs, `initialize`, accessory
//!   parsing, GRB colour order) and `Nzxt2023RgbController` (PIDs `0x2012` / `0x2021`, 3 colour
//!   channels, no fans; `_write_individual_color`: `0x22 0x10` / `0x22 0x11` / `0x22 0xA0`).
//!   <https://github.com/liquidctl/liquidctl>
//! * OpenRGB `Controllers/NZXTHue2Controller/NZXTHue2Controller.cpp` (`UpdateDeviceList`,
//!   `SendDirect`, `SendApply`, `SetChannelLEDs`, accessory-id -> LED-count switch) and
//!   `NZXTHue2Devices.cpp` (device table: `0x2012` family = 3 RGB channels, 0 fans, legacy protocol).
//!   <https://gitlab.com/CalcProgrammer1/OpenRGB>
//!
//! ```text
//! init:   write [0x10,0x01]  -> reply [0x11,0x01,...]  fw = bytes 0x11,0x12,0x13
//!         write [0x20,0x03]  -> reply [0x21,0x03,...]  [14] = channel count,
//!                                accessory id of channel c slot a = byte[15 + 6*c + a] (6 slots/channel)
//! direct: [0x22, 0x10|group, 1<<channel, 0x00, G,R,B * up to 20 LEDs]   group 0 = LEDs 0..20
//!         [0x22, 0x11, ...]                                              group 1 = LEDs 20..40
//! apply:  [0x22,0xA0, 1<<channel, 0,0x01,0,0, 0x28, 0,0, 0x80, 0, 0x32, 0,0, 0x01]
//! ```
//!
//! The LED count of a channel is the sum over its accessories (see [`accessory_led_count`]),
//! capped at the protocol maximum of 40 per channel. Unknown accessory ids contribute nothing and
//! are reported; a controller with no known LEDs is not exposed as controllable.
//! The `0x60` fan-reporting interval packets of `SmartDevice2` are fan-only and are not needed for
//! these RGB controllers (liquidctl sends them only when the device has fan channels).
//!
//! Several controllers with the same PID are handled by HID path (and serial when present).

use super::LedSink;
use anyhow::{Context, Result, bail};
use ctrlmate_protocol::RgbColor;
use hidapi::{HidApi, HidDevice};
use std::time::Duration;
use tracing::{debug, info, warn};

pub const NZXT_VID: u16 = 0x1E71;
pub const PACKET_LEN: usize = 64;
/// Protocol limit per channel (two groups of 20 LEDs).
pub const MAX_LEDS_PER_CHANNEL: usize = 40;
const LEDS_PER_GROUP: usize = 20;
const ACCESSORY_SLOTS: usize = 6;
/// Maximum frame rate per controller.
pub const MAX_FPS: u32 = 30;

/// Supported controllers: `(pid, max RGB channels, model label)`. Channel counts are upper bounds;
/// the real count comes from the device's `0x21 0x03` reply.
pub const NZXT_RGB_MODELS: &[(u16, usize, &str)] = &[
    (0x2012, 3, "NZXT RGB Controller (2023)"),
    (0x2021, 3, "NZXT RGB Controller (2023)"),
    (0x2001, 4, "NZXT HUE 2"),
    (0x2002, 2, "NZXT HUE 2 Ambient"),
    (0x2006, 2, "NZXT Smart Device V2"),
    (0x200D, 2, "NZXT Smart Device V2"),
    (0x200F, 2, "NZXT Smart Device V2"),
    (0x2009, 2, "NZXT RGB & Fan Controller"),
    (0x200E, 2, "NZXT RGB & Fan Controller"),
    (0x2010, 2, "NZXT RGB & Fan Controller"),
];

/// Backwards compatible view of the model table.
pub const NZXT_RGB_PIDS: &[(u16, u8, &str)] = &[
    (0x2012, 3, "NZXT RGB Controller (2023)"),
    (0x2021, 3, "NZXT RGB Controller (2023)"),
    (0x2001, 4, "NZXT HUE 2"),
    (0x2002, 2, "NZXT HUE 2 Ambient"),
    (0x2006, 2, "NZXT Smart Device V2"),
    (0x200D, 2, "NZXT Smart Device V2"),
    (0x200F, 2, "NZXT Smart Device V2"),
    (0x2009, 2, "NZXT RGB & Fan Controller"),
    (0x200E, 2, "NZXT RGB & Fan Controller"),
    (0x2010, 2, "NZXT RGB & Fan Controller"),
];

/// LED count of an accessory id (OpenRGB `UpdateDeviceList` switch). `None` = unknown id.
pub fn accessory_led_count(id: u8) -> Option<usize> {
    Some(match id {
        0x01 => 10,        // HUE 1 strip
        0x02 => 8,         // Aer RGB 1
        0x04 => 10,        // HUE 2 strip 10
        0x05 => 8,         // HUE 2 strip 8
        0x06 => 6,         // HUE 2 strip 6
        0x08 => 14,        // HUE 2 cable comb
        0x09 => 15,        // HUE 2 underglow 300 mm
        0x0A => 10,        // HUE 2 underglow 200 mm
        0x0B | 0x0C => 8,  // Aer RGB 2 120/140
        0x10 => 8,         // Kraken X3 ring
        0x11 => 1,         // Kraken X3 logo
        0x13 | 0x14 => 18, // F120/F140 RGB
        0x15 | 0x16 => 20, // F120/F140 RGB Duo
        0x17..=0x19 => 8,  // F120/F140 RGB Core (+ case version)
        0x1C => 16,        // F280 RGB Core
        0x1D..=0x1F => 24, // F360 Core / Kraken Elite ring / F420
        0x2A => 24,        // Kraken 2024 fans 3x8
        _ => return None,
    })
}

/// One detected RGB channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub index: usize,
    pub accessories: Vec<u8>,
    pub unknown_accessories: Vec<u8>,
    pub leds: usize,
}

/// Result of the `0x10 0x01` / `0x20 0x03` queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub firmware: Option<String>,
    pub channels: Vec<ChannelInfo>,
}

/// Parses the `0x21 0x03` lighting-info reply.
pub fn parse_lighting_info(msg: &[u8], max_channels: usize) -> Vec<ChannelInfo> {
    let reported = usize::from(msg.get(14).copied().unwrap_or(0));
    let count = reported.min(max_channels);
    (0..count)
        .map(|c| {
            let mut accessories = Vec::new();
            let mut unknown = Vec::new();
            let mut leds = 0usize;
            for a in 0..ACCESSORY_SLOTS {
                let id = msg.get(15 + c * ACCESSORY_SLOTS + a).copied().unwrap_or(0);
                if id == 0 {
                    break;
                }
                accessories.push(id);
                match accessory_led_count(id) {
                    Some(n) => leds += n,
                    None => unknown.push(id),
                }
            }
            ChannelInfo {
                index: c,
                accessories,
                unknown_accessories: unknown,
                leds: leds.min(MAX_LEDS_PER_CHANNEL),
            }
        })
        .collect()
}

pub fn build_direct_packets(
    channel: usize,
    color: RgbColor,
    led_count: usize,
) -> Vec<[u8; PACKET_LEN]> {
    let leds = led_count.min(MAX_LEDS_PER_CHANNEL);
    let mut out = Vec::new();
    let mut remaining = leds;
    let mut group = 0u8;
    while remaining > 0 {
        let n = remaining.min(LEDS_PER_GROUP);
        let mut pkt = [0u8; PACKET_LEN];
        pkt[0] = 0x22;
        pkt[1] = 0x10 | group;
        pkt[2] = 1u8 << channel;
        for i in 0..n {
            let o = 4 + i * 3;
            // GRB order (liquidctl set_color swaps to [g, r, b]; OpenRGB packs G,R,B)
            pkt[o] = color.g;
            pkt[o + 1] = color.r;
            pkt[o + 2] = color.b;
        }
        out.push(pkt);
        remaining -= n;
        group += 1;
    }
    out
}

pub fn build_apply_packet(channel: usize) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = 0x22;
    pkt[1] = 0xA0;
    pkt[2] = 1u8 << channel;
    pkt[4] = 0x01;
    pkt[7] = 0x28;
    pkt[10] = 0x80;
    pkt[12] = 0x32;
    pkt[15] = 0x01;
    pkt
}

/// Minimal HID endpoint (real device or test double).
pub trait HidIo: Send {
    fn write(&self, data: &[u8]) -> Result<usize>;
    /// Returns the number of bytes read (0 on timeout).
    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize>;
}

impl HidIo for HidDevice {
    fn write(&self, data: &[u8]) -> Result<usize> {
        Ok(HidDevice::write(self, data)?)
    }
    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize> {
        Ok(HidDevice::read_timeout(self, buf, timeout_ms)?)
    }
}

pub struct NzxtRgbBackend {
    pid: u16,
    path: String,
    serial: Option<String>,
    label: String,
    firmware: Option<String>,
    device: Box<dyn HidIo>,
    channels: Vec<ChannelInfo>,
    /// `Some(true)` once a write only worked with a leading report-id byte.
    prefixed_write: Option<bool>,
    last_color: Option<RgbColor>,
}

impl NzxtRgbBackend {
    /// Opens every supported controller (one backend per HID path).
    pub fn probe(api: &HidApi) -> Vec<Self> {
        let mut found: Vec<Self> = Vec::new();
        let mut infos: Vec<_> = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == NZXT_VID && NZXT_RGB_MODELS.iter().any(|m| m.0 == d.product_id())
            })
            .collect();
        infos.sort_by(|a, b| a.path().cmp(b.path()));
        for dev_info in infos {
            let pid = dev_info.product_id();
            let path = dev_info.path().to_string_lossy().to_string();
            if found.iter().any(|b| b.path == path) {
                continue;
            }
            let Some(&(_, max_ch, model)) = NZXT_RGB_MODELS.iter().find(|m| m.0 == pid) else {
                continue;
            };
            match api.open_path(dev_info.path()) {
                Ok(device) => {
                    let idx = found.len() + 1;
                    let serial = dev_info.serial_number().map(str::to_string);
                    match Self::from_device(
                        Box::new(device),
                        pid,
                        path.clone(),
                        serial,
                        model,
                        max_ch,
                        idx,
                    ) {
                        Ok(b) => {
                            info!(
                                "Initialized {} ({} channel(s), {} LED(s))",
                                b.name(),
                                b.channels.len(),
                                b.total_leds()
                            );
                            found.push(b);
                        }
                        Err(err) => {
                            warn!("NZXT 1E71:{pid:04X} at {path} did not initialise: {err:#}")
                        }
                    }
                }
                Err(err) => warn!(
                    "Found NZXT 1E71:{pid:04X} but cannot open it ({err}); close NZXT CAM if it is running"
                ),
            }
        }
        found
    }

    /// Runs the init handshake on an opened endpoint.
    pub fn from_device(
        device: Box<dyn HidIo>,
        pid: u16,
        path: String,
        serial: Option<String>,
        model: &str,
        max_channels: usize,
        ordinal: usize,
    ) -> Result<Self> {
        let mut be = Self {
            pid,
            path,
            serial,
            label: format!("{model} #{ordinal}"),
            firmware: None,
            device,
            channels: Vec::new(),
            prefixed_write: None,
            last_color: None,
        };
        let det = be.detect(max_channels)?;
        be.firmware = det.firmware;
        be.channels = det.channels;
        Ok(be)
    }

    fn detect(&mut self, max_channels: usize) -> Result<Detection> {
        // Drop stale reports first (liquidctl clear_enqueued_reports).
        let mut scratch = [0u8; PACKET_LEN];
        for _ in 0..32 {
            if self.device.read_timeout(&mut scratch, 0)? == 0 {
                break;
            }
        }
        self.write_report(&request(0x10, 0x01))
            .context("NZXT firmware request failed")?;
        self.write_report(&request(0x20, 0x03))
            .context("NZXT lighting-info request failed")?;

        let mut firmware = None;
        let mut channels = None;
        for _ in 0..12 {
            let n = self.device.read_timeout(&mut scratch, 500)?;
            if n < 2 {
                continue;
            }
            match (scratch[0], scratch[1]) {
                (0x11, 0x01) if n > 0x13 => {
                    firmware = Some(format!(
                        "{}.{}.{}",
                        scratch[0x11], scratch[0x12], scratch[0x13]
                    ));
                }
                (0x21, 0x03) => channels = Some(parse_lighting_info(&scratch[..n], max_channels)),
                _ => {}
            }
            if firmware.is_some() && channels.is_some() {
                break;
            }
        }
        let Some(channels) = channels else {
            bail!("no 0x21 0x03 lighting-info reply from the controller");
        };
        Ok(Detection { firmware, channels })
    }

    fn write_report(&mut self, pkt: &[u8; PACKET_LEN]) -> Result<()> {
        match self.prefixed_write {
            Some(true) => self.write_prefixed(pkt),
            Some(false) => self.device.write(pkt).map(|_| ()),
            None => match self.device.write(pkt) {
                Ok(_) => {
                    self.prefixed_write = Some(false);
                    Ok(())
                }
                Err(e64) => {
                    self.write_prefixed(pkt).with_context(|| {
                        format!("64-byte write failed ({e64:#}); report-id write failed too")
                    })?;
                    self.prefixed_write = Some(true);
                    Ok(())
                }
            },
        }
    }

    fn write_prefixed(&self, pkt: &[u8; PACKET_LEN]) -> Result<()> {
        let mut p = [0u8; PACKET_LEN + 1];
        p[1..].copy_from_slice(pkt);
        self.device.write(&p).map(|_| ())
    }

    pub fn pid(&self) -> u16 {
        self.pid
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }
    pub fn firmware(&self) -> Option<&str> {
        self.firmware.as_deref()
    }
    pub fn channels(&self) -> &[ChannelInfo] {
        &self.channels
    }
    pub fn total_leds(&self) -> usize {
        self.channels.iter().map(|c| c.leds).sum()
    }

    pub fn name(&self) -> String {
        format!("{} (1E71:{:04X})", self.label, self.pid)
    }

    /// Solid colour on every channel that has known LEDs. The first error aborts and is returned.
    pub fn apply_color(&mut self, color: RgbColor) -> Result<()> {
        if self.last_color == Some(color) {
            return Ok(());
        }
        let channels = self.channels.clone();
        for ch in channels.iter().filter(|c| c.leds > 0) {
            for pkt in build_direct_packets(ch.index, color, ch.leds) {
                self.write_report(&pkt)
                    .inspect_err(|_| self.last_color = None)
                    .with_context(|| {
                        format!("NZXT direct write failed on channel {}", ch.index + 1)
                    })?;
            }
            self.write_report(&build_apply_packet(ch.index))
                .inspect_err(|_| self.last_color = None)
                .with_context(|| format!("NZXT apply failed on channel {}", ch.index + 1))?;
        }
        self.last_color = Some(color);
        debug!(
            "{} applied RGB({}, {}, {})",
            self.label, color.r, color.g, color.b
        );
        Ok(())
    }
}

fn request(a: u8, b: u8) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = a;
    p[1] = b;
    p
}

impl LedSink for NzxtRgbBackend {
    fn name(&self) -> String {
        NzxtRgbBackend::name(self)
    }
    fn write(&mut self, color: RgbColor) -> Result<()> {
        self.apply_color(color)
    }
    fn min_interval(&self) -> Duration {
        Duration::from_millis(u64::from(1000 / MAX_FPS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Shared {
        written: Vec<Vec<u8>>,
        replies: VecDeque<Vec<u8>>,
        fail_writes: bool,
        reject_64: bool,
    }

    #[derive(Clone, Default)]
    struct MockHid(Arc<Mutex<Shared>>);

    impl HidIo for MockHid {
        fn write(&self, data: &[u8]) -> Result<usize> {
            let mut s = self.0.lock().expect("lock");
            if s.fail_writes || (s.reject_64 && data.len() == 64) {
                bail!("write failed");
            }
            s.written.push(data.to_vec());
            Ok(data.len())
        }
        fn read_timeout(&self, buf: &mut [u8], _t: i32) -> Result<usize> {
            let mut s = self.0.lock().expect("lock");
            if s.written.is_empty() {
                return Ok(0); // nothing is queued on the device before the host wrote a request
            }
            match s.replies.pop_front() {
                Some(r) => {
                    buf[..r.len()].copy_from_slice(&r);
                    Ok(r.len())
                }
                None => Ok(0),
            }
        }
    }

    fn lighting_reply() -> Vec<u8> {
        let mut r = vec![0u8; 64];
        r[0] = 0x21;
        r[1] = 0x03;
        r[14] = 3;
        // channel 0: three F120 RGB (18 LEDs each -> 54, capped at 40)
        r[15..18].copy_from_slice(&[0x13, 0x13, 0x13]);
        // channel 1: one F120 Core (8)
        r[21] = 0x17;
        // channel 2: empty
        r
    }

    fn firmware_reply() -> Vec<u8> {
        let mut r = vec![0u8; 64];
        r[0] = 0x11;
        r[1] = 0x01;
        r[0x11] = 1;
        r[0x12] = 5;
        r[0x13] = 0;
        r
    }

    fn backend(mock: &MockHid) -> NzxtRgbBackend {
        {
            let mut s = mock.0.lock().expect("lock");
            s.replies.push_back(firmware_reply());
            s.replies.push_back(lighting_reply());
        }
        NzxtRgbBackend::from_device(
            Box::new(mock.clone()),
            0x2012,
            "path-a".into(),
            Some("SN1".into()),
            "NZXT RGB Controller (2023)",
            3,
            1,
        )
        .expect("init")
    }

    #[test]
    fn detection_gives_real_led_counts() {
        let mock = MockHid::default();
        let b = backend(&mock);
        assert_eq!(b.firmware(), Some("1.5.0"));
        let leds: Vec<_> = b.channels().iter().map(|c| c.leds).collect();
        assert_eq!(leds, vec![40, 8, 0]);
        assert_eq!(b.total_leds(), 48);
        assert!(b.name().contains("NZXT RGB Controller (2023) #1"));
        // the two init requests were written
        let w = &mock.0.lock().expect("lock").written;
        assert_eq!(&w[0][..2], &[0x10, 0x01]);
        assert_eq!(&w[1][..2], &[0x20, 0x03]);
    }

    #[test]
    fn unknown_accessory_is_reported_not_guessed() {
        let mut r = lighting_reply();
        r[15] = 0x7E;
        let ch = parse_lighting_info(&r, 3);
        assert_eq!(ch[0].unknown_accessories, vec![0x7E]);
        assert_eq!(ch[0].leds, 18 + 18);
        r[14] = 9; // reported more channels than the model has
        assert_eq!(parse_lighting_info(&r, 3).len(), 3);
    }

    // Reference vectors from OpenRGB SendDirect/SendApply and liquidctl _write_individual_color.
    #[test]
    fn reference_vectors_direct_and_apply() {
        let pk = build_direct_packets(
            1,
            RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
            8,
        );
        assert_eq!(pk.len(), 1);
        assert_eq!(
            &pk[0][..10],
            &[0x22, 0x10, 0x02, 0x00, 0x22, 0x11, 0x33, 0x22, 0x11, 0x33]
        );
        assert_eq!(pk[0][4 + 8 * 3], 0); // LED 9 untouched
        let pk = build_direct_packets(0, RgbColor { r: 1, g: 2, b: 3 }, 24);
        assert_eq!(pk.len(), 2);
        assert_eq!(&pk[1][..8], &[0x22, 0x11, 0x01, 0x00, 2, 1, 3, 2]);
        assert_eq!(pk[1][4 + 4 * 3], 0); // 24 - 20 = 4 LEDs in group 1
        assert_eq!(
            build_apply_packet(2)[..16],
            [
                0x22, 0xA0, 0x04, 0x00, 0x01, 0x00, 0x00, 0x28, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00,
                0x00, 0x01
            ]
        );
    }

    #[test]
    fn apply_writes_only_channels_with_leds_and_surfaces_errors() {
        let mock = MockHid::default();
        let mut b = backend(&mock);
        mock.0.lock().expect("lock").written.clear();
        b.apply_color(RgbColor { r: 255, g: 0, b: 0 })
            .expect("apply");
        {
            let w = &mock.0.lock().expect("lock").written;
            // ch0: 2 groups + apply, ch1: 1 group + apply, ch2: nothing
            assert_eq!(w.len(), 5);
            assert_eq!(w[0][2], 0x01);
            assert_eq!(w[3][2], 0x02);
            assert_eq!(w[4][1], 0xA0);
        }
        mock.0.lock().expect("lock").fail_writes = true;
        let err = b
            .apply_color(RgbColor { r: 0, g: 255, b: 0 })
            .expect_err("must fail");
        assert!(format!("{err:#}").contains("channel 1"));
    }

    #[test]
    fn falls_back_to_report_id_prefixed_writes_once() {
        let mock = MockHid::default();
        mock.0.lock().expect("lock").reject_64 = true;
        let mut b = backend(&mock);
        b.apply_color(RgbColor { r: 1, g: 2, b: 3 }).expect("apply");
        let w = &mock.0.lock().expect("lock").written;
        assert!(w.iter().all(|p| p.len() == 65 && p[0] == 0));
    }

    #[test]
    fn missing_lighting_reply_fails_init() {
        let mock = MockHid::default();
        assert!(
            NzxtRgbBackend::from_device(Box::new(mock), 0x2012, "p".into(), None, "x", 3, 1)
                .is_err()
        );
    }
}
