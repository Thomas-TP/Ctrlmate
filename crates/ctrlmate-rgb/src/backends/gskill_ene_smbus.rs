//! G.Skill Trident Z5 (DDR5) / ENE DRAM RGB over the motherboard SMBus.
//!
//! # Protocol (verified against OpenRGB, <https://gitlab.com/CalcProgrammer1/OpenRGB>)
//!
//! * `Controllers/ENESMBusController/ENESMBusController.h`: register map
//!   (`ENE_REG_DEVICE_NAME 0x1000`, `ENE_REG_MICRON_CHECK 0x1030`, `ENE_REG_CONFIG_TABLE 0x1C00`,
//!   `ENE_REG_COLORS_DIRECT 0x8000`, `ENE_REG_COLORS_DIRECT_V2 0x8100`, `ENE_REG_DIRECT 0x8020`,
//!   `ENE_REG_APPLY 0x80A0`, `ENE_REG_SLOT_INDEX 0x80F8`, `ENE_REG_I2C_ADDRESS 0x80F9`,
//!   `ENE_CONFIG_LED_COUNT 0x02`, `ENE_APPLY_VAL 0x01`).
//! * `ENESMBusController.cpp`: register access is "write the 16-bit register (byte-swapped word) to
//!   command `0x00`, then read command `0x81` / write command `0x01` / block-write command `0x03`";
//!   colours are written **R, B, G**; per-version register sets (`AUDA0-E6K5-0101`, the Trident Z5
//!   DDR5 controller, uses the **V2** direct buffer at `0x8100`; `DIMM_LED-0102/0103` (DDR4) use `0x8000`);
//!   direct mode = write `ENE_REG_DIRECT = 1` + apply, then colour blocks (no apply per frame).
//! * `ENESMBusControllerDetect.cpp`: detection reads registers `0xA0..=0xAF` with SMBus *read byte
//!   data* and expects `0x00..=0x0F`; a `"Micron"` string at `0x1030` rejects the device; **remap**:
//!   all unconfigured DIMMs answer at `0x77`; for slot 0..8 the next address that does not ACK is
//!   assigned by writing `ENE_REG_SLOT_INDEX = slot` and `ENE_REG_I2C_ADDRESS = addr << 1` to `0x77`.
//!   This is what makes a 2-DIMM kit show up as two controllers.
//! * frugalRGB (`controllers/ene.rs`) confirms the DDR5 register sizes and R,B,G order.
//!
//! # Safety rules implemented here
//!
//! 1. **No write before the device is proven to be an ENE chip.** Presence is tested with an SMBus
//!    *receive byte* (no command, nothing written to the target); identity with *read byte data*
//!    on `0xA0..=0xAF` (a read: the command byte only selects the register, exactly as OpenRGB
//!    does). Only after the `0x00..0x0F` pattern matches do we issue the ENE register-pointer write.
//!    Earlier revisions wrote the pointer word to every address in `0x70..=0x77` first - which could
//!    reach a mux or other device - and the old doc comment claiming otherwise was wrong.
//! 2. The remap only ever writes to `0x77` after it passed the signature test, and only assigns
//!    addresses `0x70..=0x76` that did not ACK.
//! 3. SPD EEPROM / SPD hub addresses (`0x50..=0x57`) are never touched.
//! 4. Windows: every bus session holds the named mutex `Global\Access_SMBUS.HTP.Method` (the PawnIO
//!    SMBus modules document that callers must hold `\BaseNamedObjects\Access_SMBUS.HTP.Method`);
//!    Linux: only motherboard SMBus adapters (`piix4` / `i801`), never GPU DDC buses.
//! 5. Anti-cheat (architecture §7): the caller must not create this backend while an anti-cheat is
//!    running; on Windows it needs `allow_kernel_smbus` (PawnIO) and is otherwise reported as
//!    `requires_deep_hardware`.
//!
//! Linux note: boards whose BIOS claims the SMBus I/O region need `acpi_enforce_resources=lax`.

use super::LedSink;
use crate::zones::{ZONE_RAM, ZoneInfo};
use anyhow::{Result, anyhow, bail};
use ctrlmate_protocol::{RgbColor, RgbDeviceStatus};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Addresses scanned for configured ENE DRAM controllers (never SPD `0x50..0x57`).
pub const ENE_DRAM_ADDRESSES: [u8; 8] = [0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77];
/// Unconfigured DIMMs answer here until remapped.
pub const ENE_UNMAPPED_ADDRESS: u8 = 0x77;
/// Addresses a DIMM may be remapped to (subset of OpenRGB's list that cannot collide with SPD hubs).
pub const ENE_REMAP_CANDIDATES: [u8; 7] = [0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76];

const CMD_REG_ADDR: u8 = 0x00;
const CMD_WRITE_BYTE: u8 = 0x01;
const CMD_WRITE_BLOCK: u8 = 0x03;
const CMD_READ_BYTE: u8 = 0x81;
const MAX_BLOCK: usize = 32;

const REG_DEVICE_NAME: u16 = 0x1000;
const REG_MICRON_CHECK: u16 = 0x1030;
const REG_CONFIG_TABLE: u16 = 0x1C00;
const CONFIG_LED_COUNT: u16 = 0x02;
const REG_COLORS_DIRECT: u16 = 0x8000;
const REG_COLORS_DIRECT_V2: u16 = 0x8100;
const REG_DIRECT: u16 = 0x8020;
const REG_APPLY: u16 = 0x80A0;
const REG_SLOT_INDEX: u16 = 0x80F8;
const REG_I2C_ADDRESS: u16 = 0x80F9;
const APPLY_VAL: u8 = 0x01;

const MAX_ENE_LEDS: usize = 32;
/// Maximum frame rate towards the DIMMs (every frame is several SMBus transactions per module).
pub const MAX_FPS: u32 = 15;

/// Encodes a 16-bit ENE register into the SMBus word written at command `0x00`.
#[inline]
pub fn encode_ene_register_word(reg: u16) -> u16 {
    reg.swap_bytes()
}

/// Solid colour across `led_count` LEDs in ENE **R, B, G** order.
pub fn encode_ene_rbg_buffer(color: RgbColor, led_count: usize) -> Vec<u8> {
    let n = led_count.clamp(1, MAX_ENE_LEDS);
    let mut buf = Vec::with_capacity(n * 3);
    for _ in 0..n {
        buf.extend_from_slice(&[color.r, color.b, color.g]);
    }
    buf
}

/// `true` for ENE firmware strings of DRAM controllers (informational prefix check).
pub fn is_valid_ene_dram_signature(name: &str) -> bool {
    let t = name.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    t.starts_with("DIMM_LED")
        || t.starts_with("AUDA")
        || t.starts_with("AUMA")
        || t.starts_with("ENE")
}

/// Register set of a supported DRAM controller revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EneLayout {
    /// DDR4 `DIMM_LED-0102/0103`: direct colours at `0x8000`.
    V1,
    /// DDR5 `AUDA0-E6K5-0101` (Trident Z5): direct colours at `0x8100`.
    V2,
}

impl EneLayout {
    fn direct_reg(self) -> u16 {
        match self {
            Self::V1 => REG_COLORS_DIRECT,
            Self::V2 => REG_COLORS_DIRECT_V2,
        }
    }
}

/// Maps a firmware string to a verified register layout; unknown revisions are **not** guessed.
pub fn layout_for_version(version: &str) -> Option<EneLayout> {
    match version.trim_matches(|c: char| c == '\0' || c.is_whitespace()) {
        "AUDA0-E6K5-0101" => Some(EneLayout::V2),
        "DIMM_LED-0102" | "DIMM_LED-0103" => Some(EneLayout::V1),
        _ => None,
    }
}

/// Holds the platform bus lock (Windows named mutex) until dropped.
pub struct BusGuard(#[allow(dead_code)] Option<Box<dyn Send>>);

impl BusGuard {
    pub fn none() -> Self {
        Self(None)
    }
    pub fn new(inner: Box<dyn Send>) -> Self {
        Self(Some(inner))
    }
}

/// Low-level SMBus transport (Linux i2c-dev, Windows PawnIO, or a test double).
pub trait SmbusTransport: Send {
    fn adapter_name(&self) -> &str;
    /// SMBus *receive byte*: pure presence probe, nothing is written to the target.
    fn read_byte(&mut self, addr: u8) -> Result<u8>;
    fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8>;
    fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()>;
    fn write_word_data(&mut self, addr: u8, cmd: u8, word: u16) -> Result<()>;
    fn write_block_data(&mut self, addr: u8, cmd: u8, data: &[u8]) -> Result<()>;
    /// Acquires the cross-process bus lock for a group of transactions.
    fn lock_bus(&mut self) -> Result<BusGuard> {
        Ok(BusGuard::none())
    }
}

fn ene_read(t: &mut dyn SmbusTransport, addr: u8, reg: u16) -> Result<u8> {
    t.write_word_data(addr, CMD_REG_ADDR, encode_ene_register_word(reg))?;
    t.read_byte_data(addr, CMD_READ_BYTE)
}

fn ene_write(t: &mut dyn SmbusTransport, addr: u8, reg: u16, val: u8) -> Result<()> {
    t.write_word_data(addr, CMD_REG_ADDR, encode_ene_register_word(reg))?;
    t.write_byte_data(addr, CMD_WRITE_BYTE, val)
}

fn ene_write_block(t: &mut dyn SmbusTransport, addr: u8, reg: u16, data: &[u8]) -> Result<()> {
    t.write_word_data(addr, CMD_REG_ADDR, encode_ene_register_word(reg))?;
    t.write_block_data(addr, CMD_WRITE_BLOCK, data)
}

fn acks(t: &mut dyn SmbusTransport, addr: u8) -> bool {
    t.read_byte(addr).is_ok()
}

/// Read-only ENE identity test: registers `0xA0..=0xAF` must read `0x00..=0x0F`.
pub fn ene_signature_test(t: &mut dyn SmbusTransport, addr: u8) -> bool {
    (0xA0u8..=0xAF).all(|reg| t.read_byte_data(addr, reg).is_ok_and(|v| v == reg - 0xA0))
}

fn read_cstr(t: &mut dyn SmbusTransport, addr: u8, reg: u16, len: u16) -> Result<String> {
    let mut bytes = Vec::with_capacity(usize::from(len));
    for i in 0..len {
        bytes.push(ene_read(t, addr, reg + i)?);
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

/// One detected ENE DRAM controller (one DIMM).
#[derive(Debug, Clone)]
pub struct EneDimm {
    pub address: u8,
    pub version: String,
    pub led_count: usize,
    pub layout: EneLayout,
    direct_initialized: bool,
}

/// Previous name of [`EneDimm`].
pub type EneDimmSlot = EneDimm;

/// Reassigns unconfigured DIMMs from `0x77` to free addresses. Returns how many were remapped.
///
/// Requires `0x77` to pass the ENE signature test first (no blind writes).
pub fn remap_dram(t: &mut dyn SmbusTransport) -> Result<usize> {
    if !acks(t, ENE_UNMAPPED_ADDRESS) {
        return Ok(0);
    }
    if !ene_signature_test(t, ENE_UNMAPPED_ADDRESS) {
        debug!("0x77 ACKs but is not an ENE chip; remap skipped");
        return Ok(0);
    }
    if let Ok(mic) = read_cstr(t, ENE_UNMAPPED_ADDRESS, REG_MICRON_CHECK, 16)
        && mic.starts_with("Micron")
    {
        debug!("0x77 is a Micron device; remap skipped");
        return Ok(0);
    }
    let mut remapped = 0usize;
    let mut next = 0usize;
    for slot in 0u8..8 {
        if !acks(t, ENE_UNMAPPED_ADDRESS) {
            break;
        }
        // next free (non-ACKing) candidate address
        let mut target = None;
        while next < ENE_REMAP_CANDIDATES.len() {
            let cand = ENE_REMAP_CANDIDATES[next];
            next += 1;
            if !acks(t, cand) {
                target = Some(cand);
                break;
            }
        }
        let Some(addr) = target else {
            warn!("no free SMBus address left to remap ENE slot {slot}");
            break;
        };
        ene_write(t, ENE_UNMAPPED_ADDRESS, REG_SLOT_INDEX, slot)?;
        ene_write(t, ENE_UNMAPPED_ADDRESS, REG_I2C_ADDRESS, addr << 1)?;
        std::thread::sleep(Duration::from_millis(1));
        if !acks(t, addr) {
            bail!("ENE slot {slot} did not appear at 0x{addr:02X} after remap");
        }
        remapped += 1;
    }
    Ok(remapped)
}

/// Scans `0x70..=0x77` for configured ENE DRAM controllers (read-only until identified).
pub fn scan_dimms(t: &mut dyn SmbusTransport) -> Vec<EneDimm> {
    let mut out = Vec::new();
    for addr in ENE_DRAM_ADDRESSES {
        if !acks(t, addr) {
            continue;
        }
        if !ene_signature_test(t, addr) {
            debug!("SMBus 0x{addr:02X} ACKs but is not an ENE controller");
            continue;
        }
        // From here on it is an ENE chip: register-pointer writes are safe.
        if read_cstr(t, addr, REG_MICRON_CHECK, 16).is_ok_and(|s| s.starts_with("Micron")) {
            continue;
        }
        let Ok(version) = read_cstr(t, addr, REG_DEVICE_NAME, 16) else {
            continue;
        };
        let Some(layout) = layout_for_version(&version) else {
            info!("ENE controller at 0x{addr:02X} has unverified firmware '{version}', not driven");
            continue;
        };
        let leds = ene_read(t, addr, REG_CONFIG_TABLE + CONFIG_LED_COUNT).map(usize::from);
        let led_count = match leds {
            Ok(n) if (1..=MAX_ENE_LEDS).contains(&n) => n,
            other => {
                warn!(
                    "ENE controller at 0x{addr:02X}: implausible LED count {other:?}, not driven"
                );
                continue;
            }
        };
        out.push(EneDimm {
            address: addr,
            version,
            led_count,
            layout,
            direct_initialized: false,
        });
    }
    out
}

/// Probe outcome combining an optional active backend with diagnostics and the resulting zone.
pub struct GskillProbeOutcome {
    pub backend: Option<GskillEneSmbusBackend>,
    pub status: RgbDeviceStatus,
    pub zone: ZoneInfo,
}

/// Native backend driving every ENE DIMM found on one bus (zone `light_ram`).
pub struct GskillEneSmbusBackend {
    transport: Box<dyn SmbusTransport>,
    dimms: Vec<EneDimm>,
    last_color: Option<RgbColor>,
}

fn outcome_unavailable(
    transport: &str,
    controllable_via_openrgb: bool,
    reason: &str,
) -> GskillProbeOutcome {
    let (tx, controllable, reason) = if controllable_via_openrgb {
        ("OpenRGB SDK".to_string(), true, "via_openrgb".to_string())
    } else {
        (transport.to_string(), false, reason.to_string())
    };
    let status = RgbDeviceStatus {
        id: "gskill_ene_smbus".into(),
        name: "G.Skill Trident Z5 / ENE DRAM RGB".into(),
        zone_id: ZONE_RAM.into(),
        transport: tx.clone(),
        controllable,
        reason: reason.clone(),
    };
    let zone = if controllable {
        ZoneInfo::controllable(ZONE_RAM, status.name.clone(), "openrgb", tx, 0)
    } else {
        ZoneInfo::uncontrollable(
            ZONE_RAM,
            status.name.clone(),
            "gskill_ene_smbus",
            tx,
            &reason,
        )
    };
    GskillProbeOutcome {
        backend: None,
        status,
        zone,
    }
}

impl GskillEneSmbusBackend {
    /// Probes the motherboard SMBus for ENE DRAM controllers.
    ///
    /// On Windows `allow_kernel_smbus` must be `true` (PawnIO, deep-hardware mode); otherwise the
    /// result is `requires_deep_hardware` without any driver being touched.
    pub fn probe(allow_kernel_smbus: bool, has_openrgb_ram: bool) -> GskillProbeOutcome {
        #[cfg(target_os = "windows")]
        {
            if !allow_kernel_smbus {
                return outcome_unavailable(
                    "SMBus (PawnIO)",
                    has_openrgb_ram,
                    "requires_deep_hardware",
                );
            }
            match windows_pawnio::PawnIoSmbusTransport::open_first() {
                Ok(t) => Self::probe_with_transport(Box::new(t), has_openrgb_ram),
                Err(err) => {
                    warn!("PawnIO SMBus unavailable: {err:#}");
                    outcome_unavailable("SMBus (PawnIO)", has_openrgb_ram, "smbus_unavailable")
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            let _ = allow_kernel_smbus;
            for t in linux_i2c::discover_motherboard_smbus_adapters() {
                let o = Self::probe_with_transport(Box::new(t), has_openrgb_ram);
                if o.backend.is_some() {
                    return o;
                }
            }
            outcome_unavailable("Linux i2c-dev", has_openrgb_ram, "not_present")
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = (allow_kernel_smbus, has_openrgb_ram);
            outcome_unavailable("SMBus", false, "not_supported")
        }
    }

    pub fn probe_with_transport(
        mut transport: Box<dyn SmbusTransport>,
        has_openrgb_ram: bool,
    ) -> GskillProbeOutcome {
        let adapter = transport.adapter_name().to_string();
        let dimms = {
            let guard = transport.lock_bus();
            match guard {
                Ok(_g) => {
                    match remap_dram(transport.as_mut()) {
                        Ok(n) if n > 0 => info!("remapped {n} ENE DIMM(s) on {adapter}"),
                        Ok(_) => {}
                        Err(e) => warn!("ENE remap on {adapter} failed: {e:#}"),
                    }
                    scan_dimms(transport.as_mut())
                }
                Err(e) => {
                    warn!("cannot take the SMBus lock on {adapter}: {e:#}");
                    Vec::new()
                }
            }
        };
        if dimms.is_empty() {
            return outcome_unavailable(
                &format!("SMBus ({adapter})"),
                has_openrgb_ram,
                "not_present",
            );
        }
        let leds: usize = dimms.iter().map(|d| d.led_count).sum();
        for d in &dimms {
            info!(
                "ENE DRAM controller '{}' at 0x{:02X}: {} LEDs on {adapter}",
                d.version, d.address, d.led_count
            );
        }
        let name = format!("G.Skill / ENE DRAM RGB ({} DIMM)", dimms.len());
        let tx = format!("SMBus ({adapter})");
        let status = RgbDeviceStatus {
            id: "gskill_ene_smbus".into(),
            name: name.clone(),
            zone_id: ZONE_RAM.into(),
            transport: tx.clone(),
            controllable: true,
            reason: "ok".into(),
        };
        GskillProbeOutcome {
            zone: ZoneInfo::controllable(ZONE_RAM, name, "gskill_ene_smbus", tx, leds as u32),
            backend: Some(Self {
                transport,
                dimms,
                last_color: None,
            }),
            status,
        }
    }

    pub fn name(&self) -> String {
        format!(
            "G.Skill / ENE DRAM RGB ({} DIMM, {})",
            self.dimms.len(),
            self.transport.adapter_name()
        )
    }

    pub fn slot_count(&self) -> usize {
        self.dimms.len()
    }

    pub fn dimms(&self) -> &[EneDimm] {
        &self.dimms
    }

    /// Solid colour on every DIMM. All DIMMs are attempted; the first error is returned.
    pub fn apply_color(&mut self, color: RgbColor) -> Result<()> {
        if self.last_color == Some(color) {
            return Ok(());
        }
        let _guard = self.transport.lock_bus()?;
        let mut first_err: Option<anyhow::Error> = None;
        for dimm in &mut self.dimms {
            if let Err(e) = write_dimm(self.transport.as_mut(), dimm, color) {
                dimm.direct_initialized = false;
                first_err.get_or_insert(e.context(format!("ENE DIMM at 0x{:02X}", dimm.address)));
            }
        }
        match first_err {
            None => {
                self.last_color = Some(color);
                Ok(())
            }
            Some(e) => {
                self.last_color = None;
                Err(e)
            }
        }
    }
}

fn write_dimm(t: &mut dyn SmbusTransport, dimm: &mut EneDimm, color: RgbColor) -> Result<()> {
    if !dimm.direct_initialized {
        ene_write(t, dimm.address, REG_DIRECT, 0x01)?;
        ene_write(t, dimm.address, REG_APPLY, APPLY_VAL)?;
        dimm.direct_initialized = true;
    }
    let payload = encode_ene_rbg_buffer(color, dimm.led_count);
    for (i, chunk) in payload.chunks(MAX_BLOCK).enumerate() {
        let reg = dimm.layout.direct_reg() + (i * MAX_BLOCK) as u16;
        ene_write_block(t, dimm.address, reg, chunk)?;
    }
    Ok(())
}

impl LedSink for GskillEneSmbusBackend {
    fn name(&self) -> String {
        GskillEneSmbusBackend::name(self)
    }
    fn write(&mut self, color: RgbColor) -> Result<()> {
        self.apply_color(color)
    }
    fn min_interval(&self) -> Duration {
        Duration::from_millis(u64::from(1000 / MAX_FPS))
    }
}

/// `true` for adapter names of motherboard SMBus controllers (AMD PIIX4/FCH, Intel i801), `false`
/// for GPU DDC / AUX buses and everything else.
pub fn is_motherboard_smbus_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let board = lower.contains("piix4") || lower.contains("i801") || lower.contains("smbus");
    let gpu = [
        "nvidia", "amdgpu", "radeon", "i915", "nouveau", "nvkm", "ddc", "aux", "dp-",
    ]
    .iter()
    .any(|g| lower.contains(g));
    board && !gpu
}

#[cfg(target_os = "linux")]
mod linux_i2c {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::os::unix::io::AsRawFd;

    const I2C_SLAVE: libc::c_ulong = 0x0703;
    const I2C_SMBUS: libc::c_ulong = 0x0720;
    const I2C_SMBUS_READ: u8 = 1;
    const I2C_SMBUS_WRITE: u8 = 0;
    const I2C_SMBUS_BYTE: u32 = 1;
    const I2C_SMBUS_BYTE_DATA: u32 = 2;
    const I2C_SMBUS_WORD_DATA: u32 = 3;
    const I2C_SMBUS_BLOCK_DATA: u32 = 5;

    #[repr(C)]
    union I2cSmbusData {
        byte: u8,
        word: u16,
        block: [u8; 34],
    }

    #[repr(C)]
    struct I2cSmbusIoctlData {
        read_write: u8,
        command: u8,
        size: u32,
        data: *mut I2cSmbusData,
    }

    pub struct LinuxI2cSmbus {
        file: File,
        adapter: String,
    }

    impl LinuxI2cSmbus {
        fn xfer(
            &mut self,
            addr: u8,
            rw: u8,
            cmd: u8,
            size: u32,
            data: &mut I2cSmbusData,
        ) -> Result<()> {
            // SAFETY: plain ioctls on a file descriptor we own, with pointers to live stack values.
            let ret =
                unsafe { libc::ioctl(self.file.as_raw_fd(), I2C_SLAVE, libc::c_ulong::from(addr)) };
            if ret < 0 {
                return Err(anyhow!(
                    "I2C_SLAVE(0x{addr:02X}): {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut args = I2cSmbusIoctlData {
                read_write: rw,
                command: cmd,
                size,
                data: data as *mut I2cSmbusData,
            };
            // SAFETY: `args` and `data` outlive the call; layout matches `struct i2c_smbus_ioctl_data`.
            let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), I2C_SMBUS, &mut args) };
            if ret < 0 {
                return Err(anyhow!(
                    "I2C_SMBUS(0x{addr:02X}, cmd 0x{cmd:02X}): {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    impl SmbusTransport for LinuxI2cSmbus {
        fn adapter_name(&self) -> &str {
            &self.adapter
        }
        fn read_byte(&mut self, addr: u8) -> Result<u8> {
            let mut d = I2cSmbusData { byte: 0 };
            self.xfer(addr, I2C_SMBUS_READ, 0, I2C_SMBUS_BYTE, &mut d)?;
            // SAFETY: the kernel filled the `byte` member for a BYTE read.
            Ok(unsafe { d.byte })
        }
        fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8> {
            let mut d = I2cSmbusData { byte: 0 };
            self.xfer(addr, I2C_SMBUS_READ, cmd, I2C_SMBUS_BYTE_DATA, &mut d)?;
            // SAFETY: as above for BYTE_DATA.
            Ok(unsafe { d.byte })
        }
        fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()> {
            let mut d = I2cSmbusData { byte: val };
            self.xfer(addr, I2C_SMBUS_WRITE, cmd, I2C_SMBUS_BYTE_DATA, &mut d)
        }
        fn write_word_data(&mut self, addr: u8, cmd: u8, word: u16) -> Result<()> {
            let mut d = I2cSmbusData { word };
            self.xfer(addr, I2C_SMBUS_WRITE, cmd, I2C_SMBUS_WORD_DATA, &mut d)
        }
        fn write_block_data(&mut self, addr: u8, cmd: u8, payload: &[u8]) -> Result<()> {
            let len = payload.len().min(32);
            let mut block = [0u8; 34];
            block[0] = len as u8;
            block[1..=len].copy_from_slice(&payload[..len]);
            let mut d = I2cSmbusData { block };
            self.xfer(addr, I2C_SMBUS_WRITE, cmd, I2C_SMBUS_BLOCK_DATA, &mut d)
        }
    }

    pub fn discover_motherboard_smbus_adapters() -> Vec<LinuxI2cSmbus> {
        let mut adapters = Vec::new();
        let Ok(entries) = std::fs::read_dir("/sys/bus/i2c/devices") else {
            return adapters;
        };
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.starts_with("i2c-") {
                continue;
            }
            let Ok(name) = std::fs::read_to_string(entry.path().join("name")) else {
                continue;
            };
            let name = name.trim();
            if !is_motherboard_smbus_name(name) {
                continue;
            }
            let dev_path = format!("/dev/{fname}");
            match OpenOptions::new().read(true).write(true).open(&dev_path) {
                Ok(file) => adapters.push(LinuxI2cSmbus {
                    file,
                    adapter: format!("{dev_path} ({name})"),
                }),
                Err(e) => debug!("cannot open {dev_path}: {e} (i2c-dev loaded? permissions/udev?)"),
            }
        }
        adapters
    }
}

#[cfg(target_os = "windows")]
mod windows_pawnio {
    //! PawnIO SMBus transport. Interface per the PawnIO.Modules `SmbusPIIX4.p` / `SmbusI801.p`
    //! sources: `ioctl_smbus_xfer` in = `[addr, rw(0 write/1 read), cmd, protocol, data...]`
    //! (block payload = length byte + data packed little-endian in `in[4..]`, `in_size >= 9`),
    //! out[0] = value for byte/word reads; `ioctl_piix4_port_sel` in = `[port]`, out = `[previous]`.
    //! The caller must hold `\BaseNamedObjects\Access_SMBUS.HTP.Method` (= `Global\...` in Win32).
    //! The `PawnIOLib.dll` export signatures (`pawnio_open/load/execute/close`) are used as in
    //! PawnIO.Lib and could not be re-verified from a header during this change.
    use super::*;
    use libloading::Library;
    use std::ffi::{CString, c_char, c_void};
    use std::path::PathBuf;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
    use windows::core::w;

    const RW_WRITE: u64 = 0;
    const RW_READ: u64 = 1;
    const PROTO_BYTE: u64 = 1;
    const PROTO_BYTE_DATA: u64 = 2;
    const PROTO_WORD_DATA: u64 = 3;
    const PROTO_BLOCK_DATA: u64 = 5;
    const MUTEX_WAIT_MS: u32 = 5000;

    type FnOpen = unsafe extern "system" fn(*mut *mut c_void) -> i32;
    type FnLoad = unsafe extern "system" fn(*mut c_void, *const u8, usize) -> i32;
    type FnExecute = unsafe extern "system" fn(
        *mut c_void,
        *const c_char,
        *const u64,
        usize,
        *mut u64,
        usize,
        *mut usize,
    ) -> i32;
    type FnClose = unsafe extern "system" fn(*mut c_void) -> i32;

    struct SendHandle(HANDLE);
    // SAFETY: a Win32 mutex handle may be used from any thread.
    unsafe impl Send for SendHandle {}

    /// Owns the named mutex; the guard releases it on drop (same thread that acquired it).
    struct HeldMutex(HANDLE);
    // SAFETY: only moved into a BusGuard on the acquiring thread's call stack.
    unsafe impl Send for HeldMutex {}
    impl Drop for HeldMutex {
        fn drop(&mut self) {
            // SAFETY: the handle was successfully waited on by this thread.
            unsafe {
                let _ = ReleaseMutex(self.0);
            }
        }
    }

    pub struct PawnIoSmbusTransport {
        _lib: Library,
        handle: *mut c_void,
        exec_fn: FnExecute,
        close_fn: FnClose,
        adapter: String,
        mutex: SendHandle,
    }

    // SAFETY: the PawnIO handle is exclusively owned by this transport.
    unsafe impl Send for PawnIoSmbusTransport {}

    impl Drop for PawnIoSmbusTransport {
        fn drop(&mut self) {
            // SAFETY: handles are valid and owned; closed exactly once.
            unsafe {
                if !self.handle.is_null() {
                    let _ = (self.close_fn)(self.handle);
                    self.handle = std::ptr::null_mut();
                }
                let _ = CloseHandle(self.mutex.0);
            }
        }
    }

    impl PawnIoSmbusTransport {
        pub fn open_first() -> Result<Self> {
            let dll = find_dll()
                .ok_or_else(|| anyhow!("PawnIOLib.dll not found (is PawnIO installed?)"))?;
            // SAFETY: loading the vendor DLL the user installed.
            let lib = unsafe { Library::new(&dll) }
                .map_err(|e| anyhow!("loading {}: {e}", dll.display()))?;
            // SAFETY: symbols have the documented PawnIOLib signatures.
            let (open_fn, load_fn, exec_fn, close_fn): (FnOpen, FnLoad, FnExecute, FnClose) = unsafe {
                (
                    *lib.get(b"pawnio_open\0")?,
                    *lib.get(b"pawnio_load\0")?,
                    *lib.get(b"pawnio_execute\0")?,
                    *lib.get(b"pawnio_close\0")?,
                )
            };
            // SAFETY: valid NUL-terminated name; creates or opens the shared named mutex.
            let mutex = unsafe { CreateMutexW(None, false, w!("Global\\Access_SMBUS.HTP.Method")) }
                .map_err(|e| anyhow!("cannot create/open Global\\Access_SMBUS.HTP.Method: {e}"))?;

            let mut last_err =
                anyhow!("no PawnIO SMBus module blob found (SmbusPIIX4.bin / SmbusI801.bin)");
            for (module, blob) in module_blobs() {
                let mut handle: *mut c_void = std::ptr::null_mut();
                // SAFETY: out-pointer to a local.
                let hr = unsafe { open_fn(&mut handle) };
                if hr < 0 || handle.is_null() {
                    // SAFETY: handle was created by CreateMutexW above.
                    unsafe {
                        let _ = CloseHandle(mutex);
                    }
                    bail!(
                        "pawnio_open refused (HRESULT 0x{:08X}); the PawnIO driver is not installed/running or admin rights are missing",
                        hr as u32
                    );
                }
                // SAFETY: blob pointer/length are valid for the call.
                let hr = unsafe { load_fn(handle, blob.as_ptr(), blob.len()) };
                if hr < 0 {
                    // SAFETY: handle valid.
                    unsafe {
                        let _ = close_fn(handle);
                    }
                    last_err = anyhow!(
                        "pawnio_load({module}) failed (HRESULT 0x{:08X}); wrong chipset module",
                        hr as u32
                    );
                    continue;
                }
                let mut t = Self {
                    _lib: lib,
                    handle,
                    exec_fn,
                    close_fn,
                    adapter: format!("PawnIO {module}"),
                    mutex: SendHandle(mutex),
                };
                if module.contains("PIIX4") {
                    let _g = t.lock_bus()?;
                    // port 0 = primary SMBus (DIMM bus on AM4/AM5)
                    t.exec("ioctl_piix4_port_sel", &[0], 1)
                        .map_err(|e| e.context("selecting PIIX4 SMBus port 0"))?;
                }
                return Ok(t);
            }
            // SAFETY: mutex handle valid.
            unsafe {
                let _ = CloseHandle(mutex);
            }
            Err(last_err)
        }

        fn exec(&mut self, name: &str, input: &[u64], out_cells: usize) -> Result<Vec<u64>> {
            let c_name = CString::new(name)?;
            let mut out = vec![0u64; out_cells.max(1)];
            let mut ret: usize = 0;
            // SAFETY: pointers and sizes (in 64-bit cells) describe live buffers.
            let hr = unsafe {
                (self.exec_fn)(
                    self.handle,
                    c_name.as_ptr(),
                    input.as_ptr(),
                    input.len(),
                    out.as_mut_ptr(),
                    out_cells,
                    &mut ret,
                )
            };
            if hr < 0 {
                return Err(anyhow!(
                    "pawnio_execute({name}) failed (HRESULT 0x{:08X})",
                    hr as u32
                ));
            }
            out.truncate(ret.min(out.len()));
            Ok(out)
        }

        fn xfer(
            &mut self,
            addr: u8,
            rw: u64,
            cmd: u8,
            proto: u64,
            data: &[u64],
        ) -> Result<Vec<u64>> {
            let mut input = [0u64; 9];
            input[0] = u64::from(addr);
            input[1] = rw;
            input[2] = u64::from(cmd);
            input[3] = proto;
            for (i, w) in data.iter().take(5).enumerate() {
                input[4 + i] = *w;
            }
            self.exec("ioctl_smbus_xfer", &input, 5)
        }
    }

    impl SmbusTransport for PawnIoSmbusTransport {
        fn adapter_name(&self) -> &str {
            &self.adapter
        }
        fn read_byte(&mut self, addr: u8) -> Result<u8> {
            let o = self.xfer(addr, RW_READ, 0, PROTO_BYTE, &[])?;
            Ok(o.first().map_or(0, |v| (*v & 0xFF) as u8))
        }
        fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8> {
            let o = self.xfer(addr, RW_READ, cmd, PROTO_BYTE_DATA, &[])?;
            Ok(o.first().map_or(0, |v| (*v & 0xFF) as u8))
        }
        fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()> {
            self.xfer(addr, RW_WRITE, cmd, PROTO_BYTE_DATA, &[u64::from(val)])
                .map(|_| ())
        }
        fn write_word_data(&mut self, addr: u8, cmd: u8, word: u16) -> Result<()> {
            self.xfer(addr, RW_WRITE, cmd, PROTO_WORD_DATA, &[u64::from(word)])
                .map(|_| ())
        }
        fn write_block_data(&mut self, addr: u8, cmd: u8, payload: &[u8]) -> Result<()> {
            let len = payload.len().min(32);
            let mut block = [0u8; 40];
            block[0] = len as u8;
            block[1..=len].copy_from_slice(&payload[..len]);
            let mut words = [0u64; 5];
            for (i, w) in words.iter_mut().enumerate() {
                let mut b = [0u8; 8];
                b.copy_from_slice(&block[i * 8..(i + 1) * 8]);
                *w = u64::from_le_bytes(b);
            }
            self.xfer(addr, RW_WRITE, cmd, PROTO_BLOCK_DATA, &words)
                .map(|_| ())
        }
        fn lock_bus(&mut self) -> Result<BusGuard> {
            // SAFETY: valid mutex handle owned by this transport.
            let r = unsafe { WaitForSingleObject(self.mutex.0, MUTEX_WAIT_MS) };
            if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
                Ok(BusGuard::new(Box::new(HeldMutex(self.mutex.0))))
            } else {
                Err(anyhow!(
                    "timed out waiting for Global\\Access_SMBUS.HTP.Method (another tool holds the SMBus)"
                ))
            }
        }
    }

    fn find_dll() -> Option<PathBuf> {
        let mut c = vec![PathBuf::from(r"C:\Program Files\PawnIO\PawnIOLib.dll")];
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(PathBuf::from))
        {
            c.push(dir.join("PawnIOLib.dll"));
        }
        c.into_iter().find(|p| p.exists())
    }

    fn module_blobs() -> Vec<(String, Vec<u8>)> {
        let mut dirs = vec![
            PathBuf::from(r"C:\Program Files\PawnIO\modules"),
            PathBuf::from(r"C:\Program Files\PawnIO"),
        ];
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(PathBuf::from))
        {
            dirs.push(dir.join("pawnio"));
            dirs.push(dir);
        }
        let mut out = Vec::new();
        for name in ["SmbusPIIX4.bin", "SmbusI801.bin"] {
            for dir in &dirs {
                if let Ok(bytes) = std::fs::read(dir.join(name))
                    && !bytes.is_empty()
                {
                    out.push((name.trim_end_matches(".bin").to_string(), bytes));
                    break;
                }
            }
        }
        if out.is_empty() {
            warn!("PawnIO found but no SmbusPIIX4.bin / SmbusI801.bin module blob");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Chip {
        name: String,
        led_count: u8,
        slot: u8,
        pointer: u16,
        regs: BTreeMap<u16, u8>,
        micron: bool,
    }

    #[derive(Default)]
    struct World {
        /// DIMMs still answering at 0x77 (index = slot)
        unmapped: Vec<Chip>,
        mapped: BTreeMap<u8, Chip>,
        /// addresses that ACK but are not ENE (e.g. a mux)
        foreign: Vec<u8>,
        selected_slot: u8,
        foreign_writes: Vec<(u8, &'static str)>,
        log: Vec<String>,
    }

    struct MockBus(Arc<Mutex<World>>);

    fn chip(name: &str, leds: u8) -> Chip {
        Chip {
            name: name.into(),
            led_count: leds,
            ..Chip::default()
        }
    }

    impl MockBus {
        fn with<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
            f(&mut self.0.lock().expect("lock"))
        }
    }

    fn chip_read(c: &mut Chip, cmd: u8) -> u8 {
        if (0xA0..=0xAF).contains(&cmd) {
            return cmd - 0xA0;
        }
        if cmd == CMD_READ_BYTE {
            let p = c.pointer;
            if (REG_DEVICE_NAME..REG_DEVICE_NAME + 16).contains(&p) {
                return c
                    .name
                    .as_bytes()
                    .get(usize::from(p - REG_DEVICE_NAME))
                    .copied()
                    .unwrap_or(0);
            }
            if (REG_MICRON_CHECK..REG_MICRON_CHECK + 16).contains(&p) {
                let s: &[u8] = if c.micron { b"Micron" } else { b"" };
                return s
                    .get(usize::from(p - REG_MICRON_CHECK))
                    .copied()
                    .unwrap_or(0);
            }
            if p == REG_CONFIG_TABLE + CONFIG_LED_COUNT {
                return c.led_count;
            }
            return c.regs.get(&p).copied().unwrap_or(0);
        }
        0xFF
    }

    impl SmbusTransport for MockBus {
        fn adapter_name(&self) -> &str {
            "MockPIIX4"
        }
        fn read_byte(&mut self, addr: u8) -> Result<u8> {
            self.with(|w| {
                if w.foreign.contains(&addr)
                    || w.mapped.contains_key(&addr)
                    || (addr == 0x77 && !w.unmapped.is_empty())
                {
                    Ok(0)
                } else {
                    Err(anyhow!("NACK"))
                }
            })
        }
        fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8> {
            self.with(|w| {
                if w.foreign.contains(&addr) {
                    return Ok(0xFF);
                }
                if let Some(c) = w.mapped.get_mut(&addr) {
                    return Ok(chip_read(c, cmd));
                }
                if addr == 0x77 {
                    let idx = usize::from(w.selected_slot).min(w.unmapped.len().saturating_sub(1));
                    if let Some(c) = w.unmapped.get_mut(idx) {
                        return Ok(chip_read(c, cmd));
                    }
                }
                Err(anyhow!("NACK"))
            })
        }
        fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()> {
            assert_eq!(cmd, CMD_WRITE_BYTE);
            self.with(|w| {
                if w.foreign.contains(&addr) {
                    w.foreign_writes.push((addr, "byte"));
                    return Ok(());
                }
                if let Some(c) = w.mapped.get_mut(&addr) {
                    c.regs.insert(c.pointer, val);
                    w.log
                        .push(format!("w {addr:02X} {:04X}={val:02X}", c.pointer));
                    return Ok(());
                }
                if addr == 0x77 {
                    let ptr = w.unmapped.first().map_or(0, |c| c.pointer);
                    match ptr {
                        REG_SLOT_INDEX => w.selected_slot = val,
                        REG_I2C_ADDRESS => {
                            let slot = usize::from(w.selected_slot)
                                .min(w.unmapped.len().saturating_sub(1));
                            if !w.unmapped.is_empty() {
                                let mut c = w.unmapped.remove(slot);
                                c.slot = slot as u8;
                                w.mapped.insert(val >> 1, c);
                                w.selected_slot = 0;
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                Err(anyhow!("NACK"))
            })
        }
        fn write_word_data(&mut self, addr: u8, cmd: u8, word: u16) -> Result<()> {
            assert_eq!(cmd, CMD_REG_ADDR);
            self.with(|w| {
                if w.foreign.contains(&addr) {
                    w.foreign_writes.push((addr, "pointer"));
                    return Ok(());
                }
                let reg = word.swap_bytes();
                if let Some(c) = w.mapped.get_mut(&addr) {
                    c.pointer = reg;
                    return Ok(());
                }
                if addr == 0x77 {
                    for c in &mut w.unmapped {
                        c.pointer = reg;
                    }
                    return Ok(());
                }
                Err(anyhow!("NACK"))
            })
        }
        fn write_block_data(&mut self, addr: u8, cmd: u8, data: &[u8]) -> Result<()> {
            assert_eq!(cmd, CMD_WRITE_BLOCK);
            self.with(|w| {
                if let Some(c) = w.mapped.get_mut(&addr) {
                    w.log
                        .push(format!("blk {addr:02X} {:04X} {data:02X?}", c.pointer));
                    return Ok(());
                }
                Err(anyhow!("NACK"))
            })
        }
    }

    fn world(unmapped: usize, foreign: &[u8]) -> Arc<Mutex<World>> {
        let w = World {
            unmapped: (0..unmapped).map(|_| chip("AUDA0-E6K5-0101", 10)).collect(),
            foreign: foreign.to_vec(),
            ..World::default()
        };
        Arc::new(Mutex::new(w))
    }

    #[test]
    fn register_word_and_rbg_encoding() {
        assert_eq!(encode_ene_register_word(0x8020), 0x2080);
        assert_eq!(encode_ene_register_word(0x80F8), 0xF880);
        let buf = encode_ene_rbg_buffer(
            RgbColor {
                r: 10,
                g: 20,
                b: 30,
            },
            3,
        );
        assert_eq!(buf, vec![10, 30, 20, 10, 30, 20, 10, 30, 20]);
    }

    #[test]
    fn two_dimm_kit_is_remapped_into_two_controllers_without_unsafe_writes() {
        let w = world(2, &[0x70]); // a foreign device (mux) sits on 0x70
        let mut bus = MockBus(w.clone());
        assert_eq!(remap_dram(&mut bus).expect("remap"), 2);
        let found = scan_dimms(&mut bus);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].address, 0x71);
        assert_eq!(found[1].address, 0x72);
        assert!(
            found
                .iter()
                .all(|d| d.layout == EneLayout::V2 && d.led_count == 10)
        );
        let world = w.lock().expect("lock");
        assert!(
            world.foreign_writes.is_empty(),
            "wrote to foreign device: {:?}",
            world.foreign_writes
        );
    }

    #[test]
    fn non_ene_device_on_0x77_is_never_written() {
        let w = world(0, &[0x77]);
        let mut bus = MockBus(w.clone());
        assert_eq!(remap_dram(&mut bus).expect("remap"), 0);
        assert!(scan_dimms(&mut bus).is_empty());
        assert!(w.lock().expect("lock").foreign_writes.is_empty());
    }

    #[test]
    fn unknown_firmware_and_micron_are_skipped() {
        let w = world(0, &[]);
        {
            let mut g = w.lock().expect("lock");
            g.mapped.insert(0x71, chip("AUDA0-E6K5-9999", 8));
            let mut m = chip("AUDA0-E6K5-0101", 8);
            m.micron = true;
            g.mapped.insert(0x72, m);
            g.mapped.insert(0x73, chip("DIMM_LED-0102", 8));
        }
        let found = scan_dimms(&mut MockBus(w));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].layout, EneLayout::V1);
    }

    #[test]
    fn probe_and_apply_write_v2_direct_buffer_rbg() {
        let w = world(2, &[]);
        let out = GskillEneSmbusBackend::probe_with_transport(Box::new(MockBus(w.clone())), false);
        assert!(out.status.controllable);
        assert_eq!(out.zone.led_count, 20);
        let mut be = out.backend.expect("backend");
        assert_eq!(be.slot_count(), 2);
        be.apply_color(RgbColor { r: 1, g: 2, b: 3 })
            .expect("apply");
        let log = w.lock().expect("lock").log.clone();
        // direct mode enabled + applied once per DIMM, colours at 0x8100 in R,B,G order
        assert!(log.iter().any(|l| l == "w 70 8020=01"), "{log:?}");
        assert!(log.iter().any(|l| l == "w 70 80A0=01"));
        let blk = log
            .iter()
            .find(|l| l.starts_with("blk 70 8100"))
            .expect("block");
        assert!(blk.contains("01, 03, 02"), "{blk}");
        assert!(log.iter().any(|l| l.starts_with("blk 71 8100")));
        let n = log.len();
        be.apply_color(RgbColor { r: 1, g: 2, b: 3 })
            .expect("same frame");
        assert_eq!(w.lock().expect("lock").log.len(), n);
    }

    #[test]
    fn no_dimms_yields_honest_status() {
        let out =
            GskillEneSmbusBackend::probe_with_transport(Box::new(MockBus(world(0, &[]))), false);
        assert!(out.backend.is_none());
        assert!(!out.status.controllable);
        assert_eq!(out.status.reason, "not_present");
        assert!(!out.zone.controllable);
    }

    #[test]
    fn adapter_filter_excludes_gpu_buses() {
        assert!(is_motherboard_smbus_name(
            "SMBus PIIX4 adapter port 0 at 0b00"
        ));
        assert!(is_motherboard_smbus_name("SMBus I801 adapter at f000"));
        assert!(!is_motherboard_smbus_name("NVIDIA i2c adapter 1 at 1:00.0"));
        assert!(!is_motherboard_smbus_name("AMDGPU DM i2c hw bus 0"));
        assert!(!is_motherboard_smbus_name(
            "Synopsys DesignWare I2C adapter"
        ));
    }

    #[test]
    fn layouts_are_verified_only() {
        assert_eq!(layout_for_version("AUDA0-E6K5-0101"), Some(EneLayout::V2));
        assert_eq!(layout_for_version("DIMM_LED-0103"), Some(EneLayout::V1));
        assert_eq!(layout_for_version("AUDA0-E6K5-0102"), None);
        assert!(is_valid_ene_dram_signature("AUDA0-E6K5-0101"));
    }
}
