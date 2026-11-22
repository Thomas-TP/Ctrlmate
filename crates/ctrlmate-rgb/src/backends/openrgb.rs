//! Native Rust OpenRGB SDK TCP client (`127.0.0.1:6742`).
//!
//! Automatically bridges G.Skill DDR5 SMBus RAM, ASUS/MSI/Gigabyte GPUs, and 1000+ community
//! RGB devices whenever OpenRGB 1.0 SDK server is running locally (zero extra crates needed).

use anyhow::{bail, Context, Result};
use ctrlmate_protocol::RgbColor;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use tracing::{debug, info};

const ORGB_MAGIC: &[u8; 4] = b"ORGB";
const PKT_REQUEST_CONTROLLER_COUNT: u32 = 0;
const PKT_REQUEST_CONTROLLER_DATA: u32 = 1;
const PKT_REQUEST_PROTOCOL_VERSION: u32 = 40;
const PKT_SET_CLIENT_NAME: u32 = 50;
const PKT_UPDATE_LEDS: u32 = 1050;

#[derive(Debug, Clone)]
pub struct OpenRgbControllerSummary {
    pub index: u32,
    pub name: String,
    pub device_type: u32,
    pub led_count: u16,
}

pub struct OpenRgbClient {
    addr: SocketAddr,
    stream: TcpStream,
    controllers: Vec<OpenRgbControllerSummary>,
}

impl OpenRgbClient {
    /// Attempts a fast non-blocking-ish local connection to an OpenRGB SDK server (`127.0.0.1:6742`).
    pub fn try_connect(port: u16) -> Option<Self> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(120)).ok()?;
        let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));

        let mut client = Self {
            addr,
            stream,
            controllers: Vec::new(),
        };

        if let Err(err) = client.handshake() {
            debug!("OpenRGB SDK server at {addr} handshake failed: {err}");
            return None;
        }

        info!(
            "Connected to OpenRGB SDK server at {} ({} controller(s) discovered)",
            client.addr,
            client.controllers.len()
        );
        Some(client)
    }

    pub fn controllers(&self) -> &[OpenRgbControllerSummary] {
        &self.controllers
    }

    fn handshake(&mut self) -> Result<()> {
        // 1. Register client name
        let name_bytes = b"Ctrlmate\0";
        self.send_packet(0, PKT_SET_CLIENT_NAME, name_bytes)?;

        // 2. Negotiate protocol version (v3)
        let _ = self.send_packet(0, PKT_REQUEST_PROTOCOL_VERSION, &3u32.to_le_bytes());
        let _ = self.read_packet();

        // 3. Request controller count
        self.send_packet(0, PKT_REQUEST_CONTROLLER_COUNT, &[])?;
        let (_, count_payload) = self.read_packet()?;
        if count_payload.len() < 4 {
            bail!("Invalid OpenRGB controller count response");
        }
        let count = u32::from_le_bytes(count_payload[0..4].try_into()?);

        // 4. Fetch basic metadata for each controller
        self.controllers.clear();
        for idx in 0..count {
            self.send_packet(idx, PKT_REQUEST_CONTROLLER_DATA, &3u32.to_le_bytes())?;
            if let Ok((_, data)) = self.read_packet() {
                if let Some(summary) = parse_controller_summary(idx, &data) {
                    self.controllers.push(summary);
                }
            }
        }
        Ok(())
    }

    /// Sets all LEDs of a specific OpenRGB controller (or all controllers if `filter_type` is None).
    pub fn apply_color(&mut self, color: RgbColor, filter_ram_only: bool) -> Result<()> {
        let targets: Vec<(u32, u16)> = self
            .controllers
            .iter()
            .filter(|c| !filter_ram_only || c.device_type == 1) // OpenRGB DeviceType::DRAM == 1
            .map(|c| (c.index, c.led_count.max(1)))
            .collect();

        for (idx, count) in targets {
            let payload_len: u32 = 4 + 2 + u32::from(count) * 4;
            let mut body = Vec::with_capacity(payload_len as usize);
            body.extend_from_slice(&payload_len.to_le_bytes());
            body.extend_from_slice(&count.to_le_bytes());
            for _ in 0..count {
                body.extend_from_slice(&[color.r, color.g, color.b, 0x00]);
            }
            self.send_packet(idx, PKT_UPDATE_LEDS, &body)?;
        }
        Ok(())
    }

    fn send_packet(&mut self, dev_idx: u32, pkt_id: u32, payload: &[u8]) -> Result<()> {
        let mut header = [0u8; 16];
        header[0..4].copy_from_slice(ORGB_MAGIC);
        header[4..8].copy_from_slice(&dev_idx.to_le_bytes());
        header[8..12].copy_from_slice(&pkt_id.to_le_bytes());
        header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.stream
            .write_all(&header)
            .context("OpenRGB header write failed")?;
        if !payload.is_empty() {
            self.stream
                .write_all(payload)
                .context("OpenRGB payload write failed")?;
        }
        Ok(())
    }

    fn read_packet(&mut self) -> Result<(u32, Vec<u8>)> {
        let mut header = [0u8; 16];
        self.stream
            .read_exact(&mut header)
            .context("OpenRGB header read failed")?;
        if &header[0..4] != ORGB_MAGIC {
            bail!("Invalid OpenRGB magic header");
        }
        let pkt_id = u32::from_le_bytes(header[8..12].try_into()?);
        let size = u32::from_le_bytes(header[12..16].try_into()?) as usize;
        let mut payload = vec![0u8; size.min(4 * 1024 * 1024)];
        if !payload.is_empty() {
            self.stream.read_exact(&mut payload)?;
        }
        Ok((pkt_id, payload))
    }
}

fn read_u16(data: &[u8], off: &mut usize) -> Option<u16> {
    let slice = data.get(*off..*off + 2)?;
    *off += 2;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(data: &[u8], off: &mut usize) -> Option<u32> {
    let slice = data.get(*off..*off + 4)?;
    *off += 4;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_str(data: &[u8], off: &mut usize) -> Option<String> {
    let len = usize::from(read_u16(data, off)?);
    let bytes = data.get(*off..*off + len)?;
    *off += len;
    let trimmed = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    Some(String::from_utf8_lossy(trimmed).to_string())
}

fn parse_controller_summary(index: u32, data: &[u8]) -> Option<OpenRgbControllerSummary> {
    let mut off = 0usize;
    let _data_size = read_u32(data, &mut off)?;
    let device_type = read_u32(data, &mut off)?;
    let name = read_str(data, &mut off)?;
    // Skip vendor, description, version, serial, location (5 strings in protocol v3)
    let _vendor = read_str(data, &mut off);
    let _desc = read_str(data, &mut off);
    let _ver = read_str(data, &mut off);
    let _serial = read_str(data, &mut off);
    let _loc = read_str(data, &mut off);
    // Estimate safe default LED count (exact count comes after modes+zones; 32 covers DIMM/GPU/AIO safely)
    Some(OpenRgbControllerSummary {
        index,
        name,
        device_type,
        led_count: 32,
    })
}
