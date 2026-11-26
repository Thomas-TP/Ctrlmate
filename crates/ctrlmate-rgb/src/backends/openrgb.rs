//! OpenRGB SDK client (TCP, default `127.0.0.1:6742`).
//!
//! Protocol sources: OpenRGB `NetworkProtocol.h` (packet ids, `NetPacketHeader`, SDK version 6),
//! `RGBController/RGBController.cpp` (`GetDeviceDescriptionData`, `GetModeDescriptionData`,
//! `GetZoneDescriptionData`, `GetSegmentDescriptionData`, `GetLEDDescriptionData`) and
//! `RGBController::SetCustomMode` (<https://gitlab.com/CalcProgrammer1/OpenRGB>).
//!
//! * Header: `"ORGB"`, `dev_id u32`, `pkt_id u32`, `size u32` (little endian).
//! * Version handshake: the client sends `REQUEST_PROTOCOL_VERSION (40)` with its maximum; the
//!   server answers with `min(server, client)`. A server that does not answer is protocol 0.
//!   We negotiate at most [`MAX_PROTOCOL`] = 5 (protocol 6 adds zone modes / display names that
//!   Ctrlmate does not need).
//! * `REQUEST_CONTROLLER_DATA (1)` carries the negotiated version; the reply is
//!   `data_size u32` + description whose fields depend on the version (see [`parse_device`]).
//! * Colours are written with `UPDATELEDS (1050)`: `data_size u32` (includes itself), `count u16`,
//!   `count x [r g b 0]`, after `SETCUSTOMMODE (1100)` (activates Direct/Custom/Static).
//! * `DEVICE_LIST_UPDATED (100)` can arrive at any time; it marks the device list dirty.
//!
//! The client reconnects with exponential backoff (no per-frame log spam) and drains payloads it
//! does not want, so the stream never desynchronises.

use super::LedSink;
use anyhow::{Context, Result, anyhow, bail};
use ctrlmate_protocol::RgbColor;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::pipeline::lock;

const ORGB_MAGIC: &[u8; 4] = b"ORGB";
pub const MAX_PROTOCOL: u32 = 5;
const PKT_REQUEST_CONTROLLER_COUNT: u32 = 0;
const PKT_REQUEST_CONTROLLER_DATA: u32 = 1;
const PKT_REQUEST_PROTOCOL_VERSION: u32 = 40;
const PKT_SET_CLIENT_NAME: u32 = 50;
const PKT_DEVICE_LIST_UPDATED: u32 = 100;
const PKT_UPDATE_LEDS: u32 = 1050;
const PKT_SET_CUSTOM_MODE: u32 = 1100;

/// Payloads larger than this are read and discarded (never allocated).
pub const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
const MAX_FPS: u32 = 30;

/// OpenRGB `device_type` values.
pub mod device_type {
    pub const MOTHERBOARD: u32 = 0;
    pub const DRAM: u32 = 1;
    pub const GPU: u32 = 2;
    pub const COOLER: u32 = 3;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbZone {
    pub name: String,
    pub zone_type: u32,
    pub leds_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbDevice {
    pub index: u32,
    pub device_type: u32,
    pub name: String,
    pub vendor: String,
    pub description: String,
    pub version: String,
    pub serial: String,
    pub location: String,
    pub led_count: u32,
    pub zones: Vec<OpenRgbZone>,
    pub modes: Vec<String>,
}

impl OpenRgbDevice {
    /// Identity that survives index changes after a `DEVICE_LIST_UPDATED`.
    pub fn key(&self) -> DeviceKey {
        DeviceKey {
            name: self.name.clone(),
            serial: self.serial.clone(),
            location: self.location.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKey {
    pub name: String,
    pub serial: String,
    pub location: String,
}

/// Compatibility summary (previous API).
#[derive(Debug, Clone)]
pub struct OpenRgbControllerSummary {
    pub index: u32,
    pub name: String,
    pub device_type: u32,
    pub led_count: u16,
}

// ---------------------------------------------------------------- parsing

struct Cursor<'a> {
    data: &'a [u8],
    off: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.off.checked_add(n).ok_or_else(|| anyhow!("overflow"))?;
        let s = self
            .data
            .get(self.off..end)
            .ok_or_else(|| anyhow!("truncated OpenRGB description at byte {}", self.off))?;
        self.off = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// `u16` length (including the NUL) + bytes.
    fn string(&mut self) -> Result<String> {
        let len = usize::from(self.u16()?);
        let b = self.take(len)?;
        let b = b.strip_suffix(&[0]).unwrap_or(b);
        Ok(String::from_utf8_lossy(b).into_owned())
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
}

/// Parses one controller description (the bytes *after* the leading `data_size`).
///
/// Layout per OpenRGB: `type u32, name, [vendor if v>=1], description, version, serial, location,
/// num_modes u16, active_mode u32, modes[], num_zones u16, zones[], num_leds u16, leds[], colors
/// (u16 + 4 B each), [led display names (u16 + strings) if v>=5], [flags u32 if v>=5]`.
/// Mode: `name, value u32 (v<6), flags u32, speed_min, speed_max, [brightness_min, brightness_max if
/// v>=3], colors_min, colors_max, speed, [brightness if v>=3], direction, color_mode, num_colors u16,
/// colors`. Zone: `name, type u32, leds_min u32, leds_max u32, leds_count u32, matrix_len u16 + bytes,
/// [segments u16 + (name, type, start, count) if v>=4], [flags u32 if v>=5]`. LED: `name, value u32 (v<6)`.
pub fn parse_device(index: u32, data: &[u8], protocol: u32) -> Result<OpenRgbDevice> {
    let mut c = Cursor { data, off: 0 };
    let device_type = c.u32()?;
    let name = c.string()?;
    let vendor = if protocol >= 1 {
        c.string()?
    } else {
        String::new()
    };
    let description = c.string()?;
    let version = c.string()?;
    let serial = c.string()?;
    let location = c.string()?;

    let num_modes = c.u16()?;
    let _active_mode = c.u32()?;
    let mut modes = Vec::with_capacity(usize::from(num_modes));
    for _ in 0..num_modes {
        modes.push(c.string()?);
        if protocol < 6 {
            c.skip(4)?; // value
        }
        c.skip(4)?; // flags
        c.skip(8)?; // speed_min, speed_max
        if protocol >= 3 {
            c.skip(8)?; // brightness_min, brightness_max
        }
        c.skip(8)?; // colors_min, colors_max
        c.skip(4)?; // speed
        if protocol >= 3 {
            c.skip(4)?; // brightness
        }
        c.skip(8)?; // direction, color_mode
        let n = usize::from(c.u16()?);
        c.skip(n * 4)?;
    }

    let num_zones = c.u16()?;
    let mut zones = Vec::with_capacity(usize::from(num_zones));
    for _ in 0..num_zones {
        let zname = c.string()?;
        let zone_type = c.u32()?;
        let _min = c.u32()?;
        let _max = c.u32()?;
        let leds_count = c.u32()?;
        let matrix_len = usize::from(c.u16()?);
        c.skip(matrix_len)?;
        if protocol >= 4 {
            let segs = c.u16()?;
            for _ in 0..segs {
                let _ = c.string()?;
                c.skip(12)?; // type, start_idx, leds_count
            }
        }
        if protocol >= 5 {
            c.skip(4)?; // zone flags
        }
        zones.push(OpenRgbZone {
            name: zname,
            zone_type,
            leds_count,
        });
    }

    let num_leds = c.u16()?;
    for _ in 0..num_leds {
        let _ = c.string()?;
        if protocol < 6 {
            c.skip(4)?;
        }
    }
    // The LED colour array follows; we do not need it, nothing after it is required either.
    Ok(OpenRgbDevice {
        index,
        device_type,
        name,
        vendor,
        description,
        version,
        serial,
        location,
        led_count: u32::from(num_leds),
        zones,
        modes,
    })
}

// ---------------------------------------------------------------- native dedupe

/// Which native backends already drive hardware (so OpenRGB must not write to the same devices).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeCoverage {
    pub gigabyte_fusion2: bool,
    pub nzxt_hubs: bool,
    pub ene_dram: bool,
}

/// Returns the reason when `dev` is already driven by a native backend (and must be skipped).
pub fn natively_driven(dev: &OpenRgbDevice, native: NativeCoverage) -> Option<&'static str> {
    let text = format!("{} {} {}", dev.name, dev.vendor, dev.description).to_ascii_lowercase();
    if native.gigabyte_fusion2
        && text.contains("gigabyte")
        && (text.contains("fusion") || dev.device_type == device_type::MOTHERBOARD)
    {
        return Some("native_gigabyte_ite");
    }
    if native.nzxt_hubs
        && text.contains("nzxt")
        && !text.contains("kraken")
        && [
            "rgb controller",
            "rgb & fan",
            "hue",
            "smart device",
            "fan controller",
        ]
        .iter()
        .any(|k| text.contains(k))
    {
        return Some("native_nzxt_rgb");
    }
    if native.ene_dram && dev.device_type == device_type::DRAM {
        return Some("native_ene_smbus");
    }
    None
}

// ---------------------------------------------------------------- client

/// Byte stream abstraction so tests can use a loopback socket pair.
pub trait Stream: Read + Write + Send {
    fn set_timeouts(&mut self, d: Duration) -> std::io::Result<()>;
    fn set_nonblocking(&mut self, nb: bool) -> std::io::Result<()>;
    fn peek(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl Stream for TcpStream {
    fn set_timeouts(&mut self, d: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(d))?;
        self.set_write_timeout(Some(d))
    }
    fn set_nonblocking(&mut self, nb: bool) -> std::io::Result<()> {
        TcpStream::set_nonblocking(self, nb)
    }
    fn peek(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        TcpStream::peek(self, buf)
    }
}

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub struct OpenRgbClient {
    addr: SocketAddr,
    stream: Option<Box<dyn Stream>>,
    protocol: u32,
    devices: Vec<OpenRgbDevice>,
    list_dirty: bool,
    custom_mode_set: Vec<u32>,
    failures: u32,
    next_attempt: Instant,
    was_connected: bool,
}

impl OpenRgbClient {
    /// Previous API: quiet single attempt on localhost.
    pub fn try_connect(port: u16) -> Option<Self> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        match Self::connect(addr) {
            Ok(c) => Some(c),
            Err(err) => {
                debug!("OpenRGB SDK server at {addr} not available: {err:#}");
                None
            }
        }
    }

    pub fn connect(addr: SocketAddr) -> Result<Self> {
        let mut c = Self {
            addr,
            stream: None,
            protocol: 0,
            devices: Vec::new(),
            list_dirty: false,
            custom_mode_set: Vec::new(),
            failures: 0,
            next_attempt: Instant::now(),
            was_connected: false,
        };
        c.open_and_handshake()?;
        Ok(c)
    }

    fn open_and_handshake(&mut self) -> Result<()> {
        let tcp = TcpStream::connect_timeout(&self.addr, CONNECT_TIMEOUT)
            .with_context(|| format!("connect {}", self.addr))?;
        tcp.set_nodelay(true).context("TCP_NODELAY")?;
        let mut stream: Box<dyn Stream> = Box::new(tcp);
        stream.set_timeouts(IO_TIMEOUT)?;
        self.stream = Some(stream);
        self.custom_mode_set.clear();
        match self.handshake() {
            Ok(()) => {
                self.failures = 0;
                if !self.was_connected {
                    info!(
                        "Connected to OpenRGB SDK at {} (protocol {}, {} device(s))",
                        self.addr,
                        self.protocol,
                        self.devices.len()
                    );
                }
                self.was_connected = true;
                Ok(())
            }
            Err(e) => {
                self.stream = None;
                Err(e)
            }
        }
    }

    fn handshake(&mut self) -> Result<()> {
        self.send(0, PKT_SET_CLIENT_NAME, b"Ctrlmate\0")?;
        self.send(0, PKT_REQUEST_PROTOCOL_VERSION, &MAX_PROTOCOL.to_le_bytes())?;
        // Servers older than protocol 1 never answer: treat a timeout as protocol 0.
        self.protocol = match self.read_until(PKT_REQUEST_PROTOCOL_VERSION) {
            Ok(p) if p.len() >= 4 => u32::from_le_bytes([p[0], p[1], p[2], p[3]]).min(MAX_PROTOCOL),
            Ok(_) => 0,
            Err(e) if is_timeout(&e) => 0,
            Err(e) => return Err(e),
        };
        self.refresh_devices()
    }

    /// Re-reads every controller (also used after `DEVICE_LIST_UPDATED`).
    pub fn refresh_devices(&mut self) -> Result<()> {
        loop {
            self.list_dirty = false;
            self.send(0, PKT_REQUEST_CONTROLLER_COUNT, &[])?;
            let p = self.read_until(PKT_REQUEST_CONTROLLER_COUNT)?;
            if p.len() < 4 {
                bail!("short controller-count reply");
            }
            let count = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            if count > 1024 {
                bail!("implausible OpenRGB controller count {count}");
            }
            let mut devices = Vec::new();
            for idx in 0..count {
                self.send(
                    idx,
                    PKT_REQUEST_CONTROLLER_DATA,
                    &self.protocol.to_le_bytes(),
                )?;
                let data = self.read_until(PKT_REQUEST_CONTROLLER_DATA)?;
                if data.len() < 4 {
                    bail!("short controller-data reply for device {idx}");
                }
                match parse_device(idx, &data[4..], self.protocol) {
                    Ok(d) => devices.push(d),
                    Err(e) => warn!("OpenRGB device {idx} could not be parsed: {e:#}"),
                }
            }
            self.devices = devices;
            self.custom_mode_set.clear();
            if !self.list_dirty {
                return Ok(());
            }
        }
    }

    pub fn protocol(&self) -> u32 {
        self.protocol
    }

    pub fn devices(&self) -> &[OpenRgbDevice] {
        &self.devices
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Previous API.
    pub fn controllers(&self) -> Vec<OpenRgbControllerSummary> {
        self.devices
            .iter()
            .map(|d| OpenRgbControllerSummary {
                index: d.index,
                name: d.name.clone(),
                device_type: d.device_type,
                led_count: d.led_count.min(u32::from(u16::MAX)) as u16,
            })
            .collect()
    }

    pub fn find(&self, key: &DeviceKey) -> Option<&OpenRgbDevice> {
        self.devices.iter().find(|d| d.key() == *key)
    }

    /// Ensures a live connection, honouring the exponential backoff. Logs only on state changes.
    pub fn ensure_connected(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let now = Instant::now();
        if now < self.next_attempt {
            bail!(
                "OpenRGB disconnected (retry in {} ms)",
                (self.next_attempt - now).as_millis()
            );
        }
        match self.open_and_handshake() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.failures = self.failures.saturating_add(1);
                let wait =
                    (BACKOFF_MIN * 2u32.saturating_pow(self.failures.min(6))).min(BACKOFF_MAX);
                self.next_attempt = Instant::now() + wait;
                Err(e.context("OpenRGB reconnect failed"))
            }
        }
    }

    pub fn disconnect(&mut self) {
        if self.stream.take().is_some() && self.was_connected {
            info!("Disconnected from OpenRGB SDK at {}", self.addr);
            self.was_connected = false;
        }
        self.custom_mode_set.clear();
        self.next_attempt = Instant::now() + BACKOFF_MIN;
    }

    /// Processes pending asynchronous packets (`DEVICE_LIST_UPDATED`) without blocking.
    pub fn poll(&mut self) -> Result<()> {
        loop {
            let Some(stream) = self.stream.as_mut() else {
                return Ok(());
            };
            stream.set_nonblocking(true)?;
            let mut hdr = [0u8; 16];
            let peeked = stream.peek(&mut hdr);
            stream.set_nonblocking(false)?;
            match peeked {
                Ok(0) => bail!("OpenRGB closed the connection"),
                Ok(n) if n >= 16 => {
                    let (id, payload) = self.read_packet()?;
                    if id == PKT_DEVICE_LIST_UPDATED {
                        self.list_dirty = true;
                    }
                    drop(payload);
                }
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Activates the Direct/Custom/Static mode of a device (once per connection / list refresh).
    pub fn set_custom_mode(&mut self, index: u32) -> Result<()> {
        if self.custom_mode_set.contains(&index) {
            return Ok(());
        }
        self.send(index, PKT_SET_CUSTOM_MODE, &[])?;
        self.custom_mode_set.push(index);
        Ok(())
    }

    /// Writes one colour per LED.
    pub fn update_leds(&mut self, index: u32, colors: &[RgbColor]) -> Result<()> {
        let count = u16::try_from(colors.len()).context("too many LEDs")?;
        let size = 4 + 2 + colors.len() * 4;
        let mut body = Vec::with_capacity(size);
        body.extend_from_slice(&(size as u32).to_le_bytes());
        body.extend_from_slice(&count.to_le_bytes());
        for c in colors {
            body.extend_from_slice(&[c.r, c.g, c.b, 0]);
        }
        self.send(index, PKT_UPDATE_LEDS, &body)
    }

    /// Previous API: solid colour on all (or only DRAM) devices.
    pub fn apply_color(&mut self, color: RgbColor, filter_ram_only: bool) -> Result<()> {
        let targets: Vec<(u32, u32)> = self
            .devices
            .iter()
            .filter(|d| !filter_ram_only || d.device_type == device_type::DRAM)
            .map(|d| (d.index, d.led_count))
            .collect();
        for (idx, n) in targets.into_iter().filter(|t| t.1 > 0) {
            self.set_custom_mode(idx)?;
            self.update_leds(idx, &vec![color; n as usize])?;
        }
        Ok(())
    }

    /// Whole-device colour with identity lookup, reconnect and list refresh.
    pub fn write_device(&mut self, key: &DeviceKey, color: RgbColor) -> Result<()> {
        self.ensure_connected()?;
        let res = self.write_device_inner(key, color);
        if let Err(e) = &res
            && !e.to_string().starts_with("OpenRGB device")
        {
            self.disconnect();
        }
        res
    }

    fn write_device_inner(&mut self, key: &DeviceKey, color: RgbColor) -> Result<()> {
        self.poll()?;
        if self.list_dirty {
            self.refresh_devices()?;
        }
        let Some((index, leds)) = self.find(key).map(|d| (d.index, d.led_count)) else {
            bail!("OpenRGB device '{}' is no longer present", key.name);
        };
        if leds == 0 {
            bail!("OpenRGB device '{}' reports no LEDs", key.name);
        }
        self.set_custom_mode(index)?;
        self.update_leds(index, &vec![color; leds as usize])
    }

    fn send(&mut self, dev_idx: u32, pkt_id: u32, payload: &[u8]) -> Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("OpenRGB not connected"))?;
        let mut buf = Vec::with_capacity(16 + payload.len());
        buf.extend_from_slice(ORGB_MAGIC);
        buf.extend_from_slice(&dev_idx.to_le_bytes());
        buf.extend_from_slice(&pkt_id.to_le_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        stream.write_all(&buf).context("OpenRGB write failed")
    }

    fn read_packet(&mut self) -> Result<(u32, Vec<u8>)> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("OpenRGB not connected"))?;
        let mut header = [0u8; 16];
        stream
            .read_exact(&mut header)
            .context("OpenRGB header read failed")?;
        if &header[0..4] != ORGB_MAGIC {
            bail!("invalid OpenRGB magic");
        }
        let id = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let size = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;
        if size > MAX_PAYLOAD {
            // Drain so the stream stays aligned on packet boundaries.
            let mut left = size;
            let mut sink = [0u8; 16 * 1024];
            while left > 0 {
                let n = left.min(sink.len());
                stream
                    .read_exact(&mut sink[..n])
                    .context("draining oversize payload")?;
                left -= n;
            }
            return Ok((id, Vec::new()));
        }
        let mut payload = vec![0u8; size];
        stream
            .read_exact(&mut payload)
            .context("OpenRGB payload read failed")?;
        Ok((id, payload))
    }

    /// Reads packets until `expected` arrives; async notifications are handled on the way.
    fn read_until(&mut self, expected: u32) -> Result<Vec<u8>> {
        for _ in 0..64 {
            let (id, payload) = self.read_packet()?;
            if id == expected {
                return Ok(payload);
            }
            if id == PKT_DEVICE_LIST_UPDATED {
                self.list_dirty = true;
            }
        }
        bail!("OpenRGB reply {expected} never arrived")
    }
}

fn is_timeout(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )
        })
    })
}

/// One OpenRGB device exposed as a solid-colour sink.
pub struct OpenRgbDeviceSink {
    client: Arc<Mutex<OpenRgbClient>>,
    key: DeviceKey,
    label: String,
    last: Option<RgbColor>,
}

impl OpenRgbDeviceSink {
    pub fn new(client: Arc<Mutex<OpenRgbClient>>, dev: &OpenRgbDevice) -> Self {
        Self {
            client,
            key: dev.key(),
            label: format!("OpenRGB: {}", dev.name),
            last: None,
        }
    }
}

impl LedSink for OpenRgbDeviceSink {
    fn name(&self) -> String {
        self.label.clone()
    }
    fn write(&mut self, color: RgbColor) -> Result<()> {
        if self.last == Some(color) {
            return Ok(());
        }
        let res = lock(&self.client).write_device(&self.key, color);
        self.last = res.is_ok().then_some(color);
        res
    }
    fn min_interval(&self) -> Duration {
        Duration::from_millis(u64::from(1000 / MAX_FPS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    // ---- independent encoder following the documented layout (see parse_device docs)
    fn put_str(v: &mut Vec<u8>, s: &str) {
        v.extend_from_slice(&(s.len() as u16 + 1).to_le_bytes());
        v.extend_from_slice(s.as_bytes());
        v.push(0);
    }

    fn encode_device(
        protocol: u32,
        name: &str,
        dtype: u32,
        zones: &[(&str, u32)],
        leds: u16,
    ) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&dtype.to_le_bytes());
        put_str(&mut v, name);
        if protocol >= 1 {
            put_str(&mut v, "VendorCo");
        }
        put_str(&mut v, "desc");
        put_str(&mut v, "1.0");
        put_str(&mut v, "SERIAL");
        put_str(&mut v, "HID: loc");
        // two modes
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        for m in ["Direct", "Static"] {
            put_str(&mut v, m);
            if protocol < 6 {
                v.extend_from_slice(&0xFFFFu32.to_le_bytes());
            }
            v.extend_from_slice(&0u32.to_le_bytes()); // flags
            v.extend_from_slice(&[0u8; 8]); // speed min/max
            if protocol >= 3 {
                v.extend_from_slice(&[0u8; 8]);
            }
            v.extend_from_slice(&[0u8; 8]); // colors min/max
            v.extend_from_slice(&[0u8; 4]); // speed
            if protocol >= 3 {
                v.extend_from_slice(&[0u8; 4]);
            }
            v.extend_from_slice(&[0u8; 8]); // direction, color mode
            v.extend_from_slice(&1u16.to_le_bytes()); // num colors
            v.extend_from_slice(&[1, 2, 3, 0]);
        }
        v.extend_from_slice(&(zones.len() as u16).to_le_bytes());
        for (zn, zl) in zones {
            put_str(&mut v, zn);
            v.extend_from_slice(&1u32.to_le_bytes()); // type linear
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&zl.to_le_bytes());
            v.extend_from_slice(&zl.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes()); // matrix len
            if protocol >= 4 {
                v.extend_from_slice(&1u16.to_le_bytes());
                put_str(&mut v, "seg");
                v.extend_from_slice(&[0u8; 12]);
            }
            if protocol >= 5 {
                v.extend_from_slice(&[0u8; 4]);
            }
        }
        v.extend_from_slice(&leds.to_le_bytes());
        for i in 0..leds {
            put_str(&mut v, &format!("LED {i}"));
            if protocol < 6 {
                v.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        v.extend_from_slice(&leds.to_le_bytes());
        v.extend_from_slice(&vec![0u8; usize::from(leds) * 4]);
        if protocol >= 5 {
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
        }
        v
    }

    #[test]
    fn parses_real_led_counts_for_every_protocol_version() {
        for proto in 0..=5 {
            let raw = encode_device(
                proto,
                "Kraken X63",
                device_type::COOLER,
                &[("Ring", 8), ("Logo", 1)],
                9,
            );
            let d = parse_device(3, &raw, proto).expect("parse");
            assert_eq!(d.led_count, 9, "protocol {proto}");
            assert_eq!(d.zones.len(), 2);
            assert_eq!(d.zones[0].leds_count, 8);
            assert_eq!(d.modes, vec!["Direct".to_string(), "Static".to_string()]);
            assert_eq!(d.name, "Kraken X63");
            if proto >= 1 {
                assert_eq!(d.vendor, "VendorCo");
            }
        }
        assert!(parse_device(0, &[1, 2, 3], 3).is_err());
    }

    #[test]
    fn dedupes_native_devices_but_keeps_kraken() {
        let mk = |name: &str, vendor: &str, t: u32| OpenRgbDevice {
            index: 0,
            device_type: t,
            name: name.into(),
            vendor: vendor.into(),
            description: String::new(),
            version: String::new(),
            serial: String::new(),
            location: String::new(),
            led_count: 5,
            zones: vec![],
            modes: vec![],
        };
        let all = NativeCoverage {
            gigabyte_fusion2: true,
            nzxt_hubs: true,
            ene_dram: true,
        };
        assert!(natively_driven(&mk("B650 GAMING X AX V2", "Gigabyte", 0), all).is_some());
        assert!(natively_driven(&mk("NZXT RGB Controller", "NZXT", 4), all).is_some());
        assert!(natively_driven(&mk("NZXT Hue 2", "NZXT", 4), all).is_some());
        assert!(natively_driven(&mk("ENE DRAM", "ENE", device_type::DRAM), all).is_some());
        assert!(
            natively_driven(&mk("NZXT Kraken X63", "NZXT", device_type::COOLER), all).is_none()
        );
        assert!(
            natively_driven(
                &mk("NZXT RGB Controller", "NZXT", 4),
                NativeCoverage::default()
            )
            .is_none()
        );
        assert!(natively_driven(&mk("ASUS Aura GPU", "ASUS", device_type::GPU), all).is_none());
    }

    // ---- loopback mock server
    type Received = (u32, u32, Vec<u8>);
    struct Mock {
        port: u16,
        received: Arc<Mutex<Vec<Received>>>,
        handle: std::thread::JoinHandle<()>,
    }

    fn read_pkt(s: &mut TcpStream) -> Option<(u32, u32, Vec<u8>)> {
        let mut h = [0u8; 16];
        s.read_exact(&mut h).ok()?;
        let dev = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
        let id = u32::from_le_bytes([h[8], h[9], h[10], h[11]]);
        let sz = u32::from_le_bytes([h[12], h[13], h[14], h[15]]) as usize;
        let mut p = vec![0u8; sz];
        s.read_exact(&mut p).ok()?;
        Some((dev, id, p))
    }

    fn write_pkt(s: &mut TcpStream, dev: u32, id: u32, payload: &[u8]) {
        let mut b = Vec::new();
        b.extend_from_slice(b"ORGB");
        b.extend_from_slice(&dev.to_le_bytes());
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(payload);
        let _ = s.write_all(&b);
    }

    /// `noise`: send a DEVICE_LIST_UPDATED and an oversize junk packet before the version reply.
    fn spawn_mock(noise: bool) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let rec = received.clone();
        let handle = std::thread::spawn(move || {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let dev = encode_device(3, "Mock RAM", device_type::DRAM, &[("DIMM", 5)], 5);
            while let Some((d, id, p)) = read_pkt(&mut s) {
                lock(&rec).push((d, id, p.clone()));
                match id {
                    40 => {
                        assert_eq!(p, 5u32.to_le_bytes());
                        if noise {
                            write_pkt(&mut s, 0, PKT_DEVICE_LIST_UPDATED, &[]);
                            write_pkt(&mut s, 0, 999, &vec![0u8; MAX_PAYLOAD + 1024]);
                        }
                        write_pkt(&mut s, 0, 40, &3u32.to_le_bytes()); // server only speaks 3
                    }
                    0 => write_pkt(&mut s, 0, 0, &1u32.to_le_bytes()),
                    1 => {
                        assert_eq!(
                            p,
                            3u32.to_le_bytes(),
                            "data request must use the negotiated version"
                        );
                        let mut body = Vec::new();
                        body.extend_from_slice(&((dev.len() + 4) as u32).to_le_bytes());
                        body.extend_from_slice(&dev);
                        write_pkt(&mut s, d, 1, &body);
                    }
                    _ => {}
                }
            }
        });
        Mock {
            port,
            received,
            handle,
        }
    }

    #[test]
    fn handshake_negotiates_version_and_reads_led_count() {
        let mock = spawn_mock(false);
        let mut c =
            OpenRgbClient::connect(SocketAddr::from(([127, 0, 0, 1], mock.port))).expect("connect");
        assert_eq!(c.protocol(), 3);
        assert_eq!(c.devices().len(), 1);
        assert_eq!(c.devices()[0].led_count, 5);
        let key = c.devices()[0].key();
        c.write_device(&key, RgbColor { r: 1, g: 2, b: 3 })
            .expect("write");
        c.write_device(&key, RgbColor { r: 4, g: 5, b: 6 })
            .expect("write2");
        drop(c);
        let _ = mock.handle.join();
        let rec = lock(&mock.received);
        let ids: Vec<u32> = rec.iter().map(|r| r.1).collect();
        // set custom mode comes before the first update and only once
        let cm = ids
            .iter()
            .position(|&i| i == PKT_SET_CUSTOM_MODE)
            .expect("custom mode");
        let up = ids
            .iter()
            .position(|&i| i == PKT_UPDATE_LEDS)
            .expect("update");
        assert!(cm < up);
        assert_eq!(ids.iter().filter(|&&i| i == PKT_SET_CUSTOM_MODE).count(), 1);
        let upd = rec.iter().find(|r| r.1 == PKT_UPDATE_LEDS).expect("upd");
        assert_eq!(upd.2.len(), 4 + 2 + 5 * 4, "5 real LEDs, not a guessed 32");
        assert_eq!(&upd.2[6..10], &[1, 2, 3, 0]);
    }

    #[test]
    fn async_notifications_and_oversize_payloads_do_not_desync() {
        let mock = spawn_mock(true);
        let c = OpenRgbClient::connect(SocketAddr::from(([127, 0, 0, 1], mock.port)));
        // The oversize junk was drained and DEVICE_LIST_UPDATED handled without losing sync;
        // the handshake still completes.
        let c = c.expect("connect despite noise");
        assert_eq!(c.devices().len(), 1);
        drop(c);
        let _ = mock.handle.join();
    }

    #[test]
    fn reconnect_backs_off_without_hammering() {
        // Nothing listens on this port: first attempt fails, second is refused by backoff.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        let mut c = OpenRgbClient {
            addr,
            stream: None,
            protocol: 3,
            devices: vec![],
            list_dirty: false,
            custom_mode_set: vec![],
            failures: 0,
            next_attempt: Instant::now(),
            was_connected: false,
        };
        let first = c.ensure_connected().expect_err("refused");
        assert!(format!("{first:#}").contains("reconnect failed"));
        let second = c.ensure_connected().expect_err("backoff");
        assert!(second.to_string().contains("retry in"));
        assert!(c.failures >= 1);
    }
}
