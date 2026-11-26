//! Gigabyte RGB Fusion 2 motherboard controller (ITE IT5702 / IT8297 / IT5711) over USB HID.
//!
//! Zero kernel drivers: HID feature reports on Windows and Linux (`hidraw`).
//!
//! # Protocol (verified against primary sources)
//!
//! All traffic is 64-byte feature reports with report id `0xCC` in byte 0.
//!
//! * OpenRGB `Controllers/GigabyteRGBFusion2USBController/GigabyteRGBFusion2USBController.{h,cpp}`
//!   (<https://gitlab.com/CalcProgrammer1/OpenRGB>): `PktEffect`, `PktEffectApply`, `IT8297Report`,
//!   `SetLEDEffect`, `ApplyEffect`, `ResetController`, `EnableBeat`, `SetStripBuiltinEffectState`,
//!   `RGBController_GigabyteRGBFusion2USB.h` (`RGBFUSION2_BRIGHTNESS_MAX = 255`).
//! * frugalRGB `crates/frugalrgb-core/src/controllers/gigabyte.rs` (independent Rust implementation
//!   with the same byte layout; <https://github.com/emaspa/frugalRGB>).
//!
//! ```text
//! PktEffect (64 B, packed)
//!  [0]      report id 0xCC
//!  [1]      header  0x20 + led   (led 0..7)   |  0x20 with mask 0xFF = "all LEDs"
//!  [2..6]   zone0 mask, u32 LE  (1 << led)    |  0xFF (0x07FF on IT5711) for all
//!  [6..10]  zone1 mask, u32 LE  (0)
//!  [10]     reserved
//!  [11]     effect type: 0 none, 1 static, 2 pulse, 3 blink, 4 colour cycle
//!  [12]     max brightness 0..=255 (100 for pulse only - controller quirk)
//!  [13]     min brightness
//!  [14..18] colour0 = bytes B, G, R, 0   (OpenRGB RGBToBGRColor, u32 LE)
//!  [18..22] colour1
//!  [22..30] period0..3 (u16 LE)
//!  [30..34] effect params
//! Apply (PktEffectApply): [0]=0xCC [1]=0x28 [2..6]=zone mask OR-ed from the effects written, u32 LE
//! Info request: [0]=0xCC [1]=0x60  -> get_feature_report -> IT8297Report
//!  [4..8] fw (u32), [11] support_cmd_flag, [12..40] product string, [56..60] chip id
//! ```
//!
//! Brightness is composed in software by the engine (so fades are smooth), the hardware brightness
//! byte is therefore kept at 255 and the colour is sent pre-scaled.
//!
//! # Not driven (unverified)
//!
//! The addressable D_LED headers use a different report (`0x58` / `0x59` strip frames of 19 LEDs,
//! per-header colour order from the calibration words, LED count via `0x34`). The layout is
//! documented in OpenRGB but the calibration/order handling is not verified on hardware here, so it
//! is **not enabled**: [`DLED_VERIFIED`] is `false` and the zone only covers the onboard LEDs and
//! the headers that follow the standard effect packet.

use super::LedSink;
use anyhow::{Context, Result, bail};
use ctrlmate_protocol::RgbColor;
use hidapi::{HidApi, HidDevice};
use std::time::Duration;
use tracing::{debug, info, warn};

pub const GIGABYTE_ITE_VID: u16 = 0x048D;
pub const GIGABYTE_ITE_PIDS: &[u16] = &[0x5702, 0x8297, 0x5711];

/// D_LED (0x58/0x59) strip frames are intentionally disabled, see the module documentation.
pub const DLED_VERIFIED: bool = false;

pub const REPORT_ID: u8 = 0xCC;
pub const PACKET_LEN: usize = 64;
const EFFECT_STATIC: u8 = 1;
const CMD_BEAT: u8 = 0x31;
const CMD_EFFECT_DISABLE_MASK: u8 = 0x32;
const CMD_APPLY: u8 = 0x28;
const CMD_LAMP_ARRAY: u8 = 0x48;
const CMD_INFO: u8 = 0x60;
const HEADER_ALL: u8 = 0x20;

/// Maximum frame rate towards the controller (two feature reports per frame).
pub const MAX_FPS: u32 = 20;

/// Abstraction over a HID feature-report endpoint (real device or test double).
pub trait FeatureReportIo: Send {
    fn send_feature_report(&self, data: &[u8]) -> Result<()>;
    fn get_feature_report(&self, buf: &mut [u8]) -> Result<usize>;
}

impl FeatureReportIo for HidDevice {
    fn send_feature_report(&self, data: &[u8]) -> Result<()> {
        HidDevice::send_feature_report(self, data)?;
        Ok(())
    }
    fn get_feature_report(&self, buf: &mut [u8]) -> Result<usize> {
        Ok(HidDevice::get_feature_report(self, buf)?)
    }
}

/// Decoded `IT8297Report`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItInfo {
    pub product: u8,
    pub firmware: [u8; 4],
    pub support_cmd_flag: u8,
    pub product_string: String,
    pub chip_id: u32,
}

/// Parses the response to the `0x60` info request.
pub fn parse_info(resp: &[u8]) -> Option<ItInfo> {
    if resp.len() < 60 || resp[0] != REPORT_ID {
        return None;
    }
    let product_string: String = resp[12..40]
        .iter()
        .take_while(|&&b| b != 0)
        .filter(|&&b| b.is_ascii_graphic() || b == b' ')
        .map(|&b| b as char)
        .collect();
    Some(ItInfo {
        product: resp[1],
        firmware: [resp[4], resp[5], resp[6], resp[7]],
        support_cmd_flag: resp[11],
        product_string: product_string.trim().to_string(),
        chip_id: u32::from_le_bytes([resp[56], resp[57], resp[58], resp[59]]),
    })
}

/// Static-colour effect packet for one LED (`0..8`) or all LEDs (`None`), full hardware brightness.
pub fn build_effect_packet(pid: u16, led: Option<u8>, color: RgbColor) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = REPORT_ID;
    let mask: u32 = match led {
        Some(l) => {
            pkt[1] = HEADER_ALL + l.min(7);
            1u32 << l.min(7)
        }
        None => {
            pkt[1] = HEADER_ALL;
            if pid == 0x5711 { 0x07FF } else { 0xFF }
        }
    };
    pkt[2..6].copy_from_slice(&mask.to_le_bytes());
    pkt[11] = EFFECT_STATIC;
    pkt[12] = 0xFF;
    pkt[13] = 0x00;
    pkt[14] = color.b;
    pkt[15] = color.g;
    pkt[16] = color.r;
    pkt
}

/// `PktEffectApply` with the zone mask of the effects just written.
pub fn build_apply_packet(zone_mask: u32) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = REPORT_ID;
    pkt[1] = CMD_APPLY;
    pkt[2..6].copy_from_slice(&zone_mask.to_le_bytes());
    pkt
}

/// Simple command report `[0xCC, cmd, a, b]` (OpenRGB `SendCCReport`).
pub fn build_command(cmd: u8, a: u8, b: u8) -> [u8; PACKET_LEN] {
    let mut pkt = [0u8; PACKET_LEN];
    pkt[0] = REPORT_ID;
    pkt[1] = cmd;
    pkt[2] = a;
    pkt[3] = b;
    pkt
}

pub struct GigabyteIteBackend {
    pid: u16,
    path: String,
    device: Box<dyn FeatureReportIo>,
    info: Option<ItInfo>,
    taken_over: bool,
    last_color: Option<RgbColor>,
}

impl GigabyteIteBackend {
    pub fn probe(api: &HidApi) -> Option<Self> {
        for dev_info in api.device_list() {
            if dev_info.vendor_id() != GIGABYTE_ITE_VID
                || !GIGABYTE_ITE_PIDS.contains(&dev_info.product_id())
            {
                continue;
            }
            // Windows exposes several collections; the RGB one is usage page 0xFF89 / usage 0xCC.
            // Linux hidraw reports usage page 0 for the node, accept it.
            let up = dev_info.usage_page();
            let u = dev_info.usage();
            if up != 0 && !(up == 0xFF89 && u == 0x00CC) {
                continue;
            }
            let pid = dev_info.product_id();
            let path = dev_info.path().to_string_lossy().to_string();
            match api.open_path(dev_info.path()) {
                Ok(device) => match Self::from_device(Box::new(device), pid, path) {
                    Ok(b) => {
                        info!("Initialized Gigabyte RGB Fusion 2 backend: {}", b.name());
                        return Some(b);
                    }
                    Err(err) => warn!(
                        "Gigabyte ITE 048D:{pid:04X} did not answer the info request: {err:#}"
                    ),
                },
                Err(err) => warn!(
                    "Found Gigabyte ITE controller 048D:{pid:04X} but cannot open it ({err}); close GIGABYTE Control Center / RGB Fusion"
                ),
            }
        }
        None
    }

    /// Builds the backend on an already opened endpoint; reads the info report.
    pub fn from_device(device: Box<dyn FeatureReportIo>, pid: u16, path: String) -> Result<Self> {
        device
            .send_feature_report(&build_command(CMD_INFO, 0, 0))
            .context("IT5702 info request (0xCC 0x60) failed")?;
        let mut resp = [0u8; PACKET_LEN];
        resp[0] = REPORT_ID;
        let len = device
            .get_feature_report(&mut resp)
            .context("IT5702 info response read failed")?;
        let Some(info) = parse_info(&resp[..len.min(PACKET_LEN)]) else {
            bail!("unexpected IT5702 info response ({len} bytes)");
        };
        Ok(Self {
            pid,
            path,
            device,
            info: Some(info),
            taken_over: false,
            last_color: None,
        })
    }

    pub fn pid(&self) -> u16 {
        self.pid
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn info(&self) -> Option<&ItInfo> {
        self.info.as_ref()
    }

    pub fn name(&self) -> String {
        let label = self
            .info
            .as_ref()
            .map(|i| i.product_string.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("Gigabyte RGB Fusion 2");
        format!("{label} (048D:{:04X})", self.pid)
    }

    /// Releases the board from the vendor lighting engine (first write only), mirroring OpenRGB's
    /// `ResetController` + `EnableBeat(false)` and frugalRGB's take-over sequence.
    fn take_over(&mut self) -> Result<()> {
        if self
            .info
            .as_ref()
            .is_some_and(|i| i.support_cmd_flag & 0x02 != 0)
        {
            self.device
                .send_feature_report(&build_command(CMD_LAMP_ARRAY, 0, 0))
                .context("IT5702 leave lamp-array mode failed")?;
        }
        for reg in 0x20u8..=0x27 {
            self.device
                .send_feature_report(&build_command(reg, 0, 0))
                .with_context(|| format!("IT5702 reset of effect register 0x{reg:02X} failed"))?;
        }
        if self.pid == 0x5711 {
            for reg in 0x90u8..=0x92 {
                self.device.send_feature_report(&build_command(reg, 0, 0))?;
            }
        }
        let mut fast = build_command(CMD_APPLY, 0xFF, 0x00);
        if self.pid == 0x5711 {
            fast[3] = 0x07;
        }
        self.device.send_feature_report(&fast)?;
        self.device
            .send_feature_report(&build_command(CMD_BEAT, 0, 0))?;
        // 0 = every built-in effect enabled (a static effect on D_LED headers needs them on).
        self.device
            .send_feature_report(&build_command(CMD_EFFECT_DISABLE_MASK, 0, 0))?;
        self.taken_over = true;
        Ok(())
    }

    /// Sets every motherboard LED to one solid colour. Errors are returned, never swallowed.
    pub fn apply_color(&mut self, color: RgbColor) -> Result<()> {
        if self.last_color == Some(color) {
            return Ok(());
        }
        if !self.taken_over {
            self.take_over()?;
        }
        let eff = build_effect_packet(self.pid, None, color);
        let mask = u32::from_le_bytes([eff[2], eff[3], eff[4], eff[5]]);
        let res = self
            .device
            .send_feature_report(&eff)
            .context("IT5702 static effect write failed")
            .and_then(|()| {
                self.device
                    .send_feature_report(&build_apply_packet(mask))
                    .context("IT5702 apply (0x28) failed")
            });
        match res {
            Ok(()) => {
                self.last_color = Some(color);
                debug!("IT5702 applied RGB({}, {}, {})", color.r, color.g, color.b);
                Ok(())
            }
            Err(e) => {
                self.last_color = None;
                Err(e)
            }
        }
    }
}

impl LedSink for GigabyteIteBackend {
    fn name(&self) -> String {
        GigabyteIteBackend::name(self)
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
    use std::sync::{Arc, Mutex};

    // Reference vectors derived by hand from the PktEffect / PktEffectApply layouts quoted in the
    // module documentation (OpenRGB GigabyteRGBFusion2USBController.h), NOT from this crate's builders.
    #[test]
    fn reference_vector_static_all_leds() {
        let mut expected = [0u8; 64];
        expected[..18].copy_from_slice(&[
            0xCC, 0x20, // report id, header "all"
            0xFF, 0x00, 0x00, 0x00, // zone0 = 0xFF
            0x00, 0x00, 0x00, 0x00, // zone1
            0x00, // reserved0
            0x01, // effect static
            0xFF, // max brightness
            0x00, // min brightness
            0x33, 0x22, 0x11, 0x00, // colour0 = B G R 0 for RGB(0x11,0x22,0x33)
        ]);
        let got = build_effect_packet(
            0x5702,
            None,
            RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn reference_vector_single_led_and_5711() {
        // led 2: header 0x22, zone0 = 1<<2
        let got = build_effect_packet(0x5702, Some(2), RgbColor { r: 1, g: 2, b: 3 });
        assert_eq!(&got[..6], &[0xCC, 0x22, 0x04, 0, 0, 0]);
        assert_eq!(&got[11..17], &[0x01, 0xFF, 0x00, 3, 2, 1]);
        // IT5711 "all" mask is 0x07FF
        let all = build_effect_packet(0x5711, None, RgbColor { r: 0, g: 0, b: 0 });
        assert_eq!(&all[2..6], &[0xFF, 0x07, 0x00, 0x00]);
    }

    #[test]
    fn reference_vector_apply_and_commands() {
        let mut expected = [0u8; 64];
        expected[..6].copy_from_slice(&[0xCC, 0x28, 0xFF, 0x00, 0x00, 0x00]);
        assert_eq!(build_apply_packet(0xFF), expected);
        assert_eq!(&build_command(0x31, 0, 0)[..4], &[0xCC, 0x31, 0, 0]);
    }

    #[test]
    fn info_report_layout() {
        let mut r = [0u8; 64];
        r[0] = 0xCC;
        r[1] = 0x57; // product
        r[4..8].copy_from_slice(&[0, 10, 0, 1]);
        r[11] = 0x02;
        r[12..12 + 25].copy_from_slice(b"IT5702-GIGABYTE V1.0.10.0");
        r[56..60].copy_from_slice(&0x0000_5702u32.to_le_bytes());
        let info = parse_info(&r).expect("parse");
        assert_eq!(info.product_string, "IT5702-GIGABYTE V1.0.10.0");
        assert_eq!(info.support_cmd_flag, 2);
        assert_eq!(info.chip_id, 0x5702);
        assert!(parse_info(&r[..30]).is_none());
    }

    #[derive(Default, Clone)]
    struct MockHid {
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
        fail: Arc<Mutex<bool>>,
    }

    impl FeatureReportIo for MockHid {
        fn send_feature_report(&self, data: &[u8]) -> Result<()> {
            if *self.fail.lock().expect("lock") {
                bail!("hid error");
            }
            self.sent.lock().expect("lock").push(data.to_vec());
            Ok(())
        }
        fn get_feature_report(&self, buf: &mut [u8]) -> Result<usize> {
            buf.fill(0);
            buf[0] = 0xCC;
            buf[11] = 0x02;
            buf[12..12 + 4].copy_from_slice(b"IT57");
            Ok(64)
        }
    }

    #[test]
    fn apply_sequence_and_error_surfacing() {
        let hid = MockHid::default();
        let mut b = GigabyteIteBackend::from_device(Box::new(hid.clone()), 0x5702, "p".into())
            .expect("init");
        b.apply_color(RgbColor { r: 9, g: 8, b: 7 }).expect("apply");
        {
            let sent = hid.sent.lock().expect("lock");
            // info + lamp-array + 8 resets + fast apply + beat + 0x32 + effect + apply
            assert_eq!(sent.len(), 1 + 1 + 8 + 1 + 1 + 1 + 1 + 1);
            assert_eq!(sent[1][1], 0x48);
            let effect = &sent[sent.len() - 2];
            assert_eq!(&effect[14..17], &[7, 8, 9]);
            assert_eq!(sent[sent.len() - 1][1], 0x28);
        }
        // identical frame: nothing sent
        let n = hid.sent.lock().expect("lock").len();
        b.apply_color(RgbColor { r: 9, g: 8, b: 7 }).expect("same");
        assert_eq!(hid.sent.lock().expect("lock").len(), n);
        // failure is an Err and the frame is retried next time
        *hid.fail.lock().expect("lock") = true;
        assert!(b.apply_color(RgbColor { r: 1, g: 1, b: 1 }).is_err());
        *hid.fail.lock().expect("lock") = false;
        b.apply_color(RgbColor { r: 1, g: 1, b: 1 }).expect("retry");
    }
}
