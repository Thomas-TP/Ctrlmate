//! Native G.Skill Trident Z5 / DDR5 (ENE SMBus DRAM) RGB controller driver.
//!
//! Protocol overview (ENE SMBus DRAM `0x70..=0x77`):
//! - 16-bit internal ENE register addresses are written big-endian to SMBus command `0x00`.
//! - Byte read uses SMBus command `0x81`; byte write uses `0x01`; block write uses `0x03`.
//! - Probe ONLY scans `0x70..=0x77` and verifies the ENE ASCII signature at `0x1000`
//!   (`DIMM_LED...` / `AUDA0...` / `AUMA0...`) before issuing any write. It NEVER touches
//!   SPD EEPROM addresses (`0x50..=0x57`).
//! - Direct color buffer lives at `0x8000` in **RBG** channel order (`[R, B, G]`), direct mode
//!   select at `0x8020 = 0x01`, and commit/apply at `0x80A0 = 0x01`.
//!
//! Transport layer:
//! - Linux: native `/dev/i2c-*` (`I2C_SMBUS` ioctl) strictly filtered to motherboard SMBus
//!   adapters (`PIIX4` / `I801` / `SMBus`) — never GPU DDC buses.
//! - Windows: opt-in dynamic `PawnIOLib.dll` loader (`SmbusPIIX4.bin` / `SmbusI801.bin`),
//!   **strictly disabled** in `SecurityMode::ZeroKernelSafe` (Anti-Cheat Safe mode).

use anyhow::{anyhow, Result};
use ctrlmate_protocol::{RgbColor, RgbDeviceStatus};
use tracing::{debug, info, warn};

/// SMBus slave addresses used by ENE DRAM RGB controllers (never touches 0x50..0x57 SPD).
pub const ENE_DRAM_ADDRESSES: [u8; 8] = [0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77];

const ENE_CMD_REG_ADDR: u8 = 0x00;
const ENE_CMD_WRITE_BYTE: u8 = 0x01;
const ENE_CMD_WRITE_BLOCK: u8 = 0x03;
const ENE_CMD_READ_BYTE: u8 = 0x81;

const ENE_REG_DEVICE_NAME: u16 = 0x1000;
const ENE_REG_LED_COUNT: u16 = 0x1C02;
const ENE_REG_COLORS_DIRECT: u16 = 0x8000;
const ENE_REG_DIRECT_SELECT: u16 = 0x8020;
const ENE_REG_MODE: u16 = 0x8021;
const ENE_REG_APPLY: u16 = 0x80A0;

const DEFAULT_TRIDENT_Z5_LEDS: usize = 8;
const MAX_ENE_LEDS: usize = 16;

/// Encodes a 16-bit ENE register into the SMBus word payload expected at command `0x00`.
/// SMBus word transfers are little-endian on the wire, whereas ENE expects `[reg_hi, reg_lo]`.
#[inline]
pub fn encode_ene_register_word(reg: u16) -> u16 {
    reg.swap_bytes()
}

/// Encodes a solid color across `led_count` ENE DRAM LEDs in **RBG** byte order (`[R, B, G]`).
pub fn encode_ene_rbg_buffer(color: RgbColor, led_count: usize) -> Vec<u8> {
    let clamped = led_count.clamp(1, MAX_ENE_LEDS);
    let mut buf = Vec::with_capacity(clamped * 3);
    for _ in 0..clamped {
        buf.push(color.r);
        buf.push(color.b);
        buf.push(color.g);
    }
    buf
}

/// Validates whether a 16-byte device name read from ENE `0x1000` is a known ENE DRAM RGB chip.
pub fn is_valid_ene_dram_signature(name: &str) -> bool {
    let trimmed = name.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    trimmed.starts_with("DIMM_LED")
        || trimmed.starts_with("AUDA")
        || trimmed.starts_with("AUMA")
        || trimmed.starts_with("ENE")
}

/// Low-level SMBus transport abstraction (Linux i2c-dev or Windows PawnIO).
pub trait SmbusTransport: Send {
    fn adapter_name(&self) -> &str;
    fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8>;
    fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()>;
    fn write_word_data(&mut self, addr: u8, cmd: u8, word: u16) -> Result<()>;
    fn write_block_data(&mut self, addr: u8, cmd: u8, data: &[u8]) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct EneDimmSlot {
    pub address: u8,
    pub device_name: String,
    pub led_count: usize,
    pub direct_initialized: bool,
}

/// Active native ENE SMBus DRAM backend controlling all discovered G.Skill / ENE DIMMs.
pub struct GskillEneSmbusBackend {
    transport: Box<dyn SmbusTransport>,
    slots: Vec<EneDimmSlot>,
}

/// Probe outcome combining an optional active backend with user-facing diagnostics.
pub struct GskillProbeOutcome {
    pub backend: Option<GskillEneSmbusBackend>,
    pub status: RgbDeviceStatus,
}

impl GskillEneSmbusBackend {
    /// Probes motherboard SMBus for ENE DRAM RGB controllers (e.g. G.Skill Trident Z5 DDR5).
    ///
    /// On Windows, `allow_kernel_smbus` MUST be `true` (`SecurityMode::DeepHardwarePawnIo`)
    /// before PawnIO is ever touched; in `ZeroKernelSafe` mode this returns a clean status
    /// explaining how to enable it (or use OpenRGB SDK) without loading any kernel driver.
    pub fn probe(allow_kernel_smbus: bool, has_openrgb_ram: bool) -> GskillProbeOutcome {
        #[cfg(target_os = "windows")]
        {
            if !allow_kernel_smbus {
                let (controllable, transport, reason) = if has_openrgb_ram {
                    (
                        true,
                        "OpenRGB SDK (127.0.0.1:6742)".to_string(),
                        "Contrôlé via OpenRGB SDK (mode Anti-Cheat Safe préservé).".to_string(),
                    )
                } else {
                    (
                        false,
                        "SMBus DDR5 (verrouillé par mode Anti-Cheat Safe)".to_string(),
                        "G.Skill Trident Z5 utilise le bus SMBus carte mère. Activez « Deep Hardware (PawnIO) » dans Matériel (avec PawnIO installé + service Windows) ou lancez OpenRGB SDK."
                            .to_string(),
                    )
                };
                return GskillProbeOutcome {
                    backend: None,
                    status: RgbDeviceStatus {
                        id: "gskill_ene_smbus".into(),
                        name: "G.Skill Trident Z5 / ENE DRAM RGB".into(),
                        zone_id: "light_ram".into(),
                        transport,
                        controllable,
                        reason,
                    },
                };
            }

            match windows_pawnio::PawnIoSmbusTransport::open_first() {
                Ok(transport) => Self::probe_with_transport(Box::new(transport), has_openrgb_ram),
                Err(err) => {
                    let (controllable, transport, reason) = if has_openrgb_ram {
                        (
                            true,
                            "OpenRGB SDK (127.0.0.1:6742)".to_string(),
                            format!("Contrôlé via OpenRGB SDK (PawnIO SMBus indisponible : {err})."),
                        )
                    } else {
                        (
                            false,
                            "SMBus DDR5 (PawnIO requis)".to_string(),
                            format!(
                                "Impossible d'ouvrir le bus SMBus via PawnIO : {err}. Installez PawnIO (https://pawnio.eu) et exécutez ctrlmated en service Windows ou administrateur."
                            ),
                        )
                    };
                    GskillProbeOutcome {
                        backend: None,
                        status: RgbDeviceStatus {
                            id: "gskill_ene_smbus".into(),
                            name: "G.Skill Trident Z5 / ENE DRAM RGB".into(),
                            zone_id: "light_ram".into(),
                            transport,
                            controllable,
                            reason,
                        },
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            let _ = allow_kernel_smbus;
            for transport in linux_i2c::discover_motherboard_smbus_adapters() {
                let outcome = Self::probe_with_transport(Box::new(transport), has_openrgb_ram);
                if outcome.backend.is_some() {
                    return outcome;
                }
            }
            let (controllable, transport, reason) = if has_openrgb_ram {
                (
                    true,
                    "OpenRGB SDK (127.0.0.1:6742)".to_string(),
                    "Contrôlé via OpenRGB SDK.".to_string(),
                )
            } else {
                (
                    false,
                    "Linux /dev/i2c-* (i2c-dev)".to_string(),
                    "Aucune barrette ENE DRAM détectée sur /dev/i2c-* (vérifiez `modprobe i2c-dev i2c-piix4` et les permissions udev)."
                        .to_string(),
                )
            };
            GskillProbeOutcome {
                backend: None,
                status: RgbDeviceStatus {
                    id: "gskill_ene_smbus".into(),
                    name: "G.Skill Trident Z5 / ENE DRAM RGB".into(),
                    zone_id: "light_ram".into(),
                    transport,
                    controllable,
                    reason,
                },
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = (allow_kernel_smbus, has_openrgb_ram);
            GskillProbeOutcome {
                backend: None,
                status: RgbDeviceStatus {
                    id: "gskill_ene_smbus".into(),
                    name: "G.Skill Trident Z5 / ENE DRAM RGB".into(),
                    zone_id: "light_ram".into(),
                    transport: "SMBus".into(),
                    controllable: false,
                    reason: "Non supporté sur cette plateforme.".into(),
                },
            }
        }
    }

    pub fn probe_with_transport(
        mut transport: Box<dyn SmbusTransport>,
        has_openrgb_ram: bool,
    ) -> GskillProbeOutcome {
        let mut slots = Vec::new();
        for addr in ENE_DRAM_ADDRESSES {
            if let Some(slot) = probe_ene_slot(transport.as_mut(), addr) {
                info!(
                    "Detected ENE DRAM RGB controller at 0x{addr:02X} ('{}', {} LEDs) on {}",
                    slot.device_name,
                    slot.led_count,
                    transport.adapter_name()
                );
                slots.push(slot);
            }
        }

        if slots.is_empty() {
            let adapter = transport.adapter_name().to_string();
            let (controllable, tx, reason) = if has_openrgb_ram {
                (
                    true,
                    "OpenRGB SDK (127.0.0.1:6742)".to_string(),
                    format!("Contrôlé via OpenRGB SDK (aucun ENE SMBus détecté sur {adapter})."),
                )
            } else {
                (
                    false,
                    format!("SMBus ({adapter})"),
                    format!(
                        "Bus SMBus {adapter} ouvert, mais aucun contrôleur ENE DRAM (0x70–0x77) n'a répondu (fermez G.Skill Lighting / GIGABYTE Control Center puis rescannez)."
                    ),
                )
            };
            return GskillProbeOutcome {
                backend: None,
                status: RgbDeviceStatus {
                    id: "gskill_ene_smbus".into(),
                    name: "G.Skill Trident Z5 / ENE DRAM RGB".into(),
                    zone_id: "light_ram".into(),
                    transport: tx,
                    controllable,
                    reason,
                },
            };
        }

        let count = slots.len();
        let adapter = transport.adapter_name().to_string();
        let summary_addrs: Vec<String> = slots.iter().map(|s| format!("0x{:02X}", s.address)).collect();
        let status = RgbDeviceStatus {
            id: "gskill_ene_smbus".into(),
            name: format!("G.Skill / ENE DRAM RGB ({count} barrette(s))"),
            zone_id: "light_ram".into(),
            transport: format!("SMBus natif ({adapter})"),
            controllable: true,
            reason: format!("Actif sur adresses {}", summary_addrs.join(", ")),
        };

        GskillProbeOutcome {
            backend: Some(Self { transport, slots }),
            status,
        }
    }

    pub fn name(&self) -> String {
        format!(
            "G.Skill / ENE DRAM RGB ({} DIMM, {})",
            self.slots.len(),
            self.transport.adapter_name()
        )
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Applies a solid color to all discovered ENE DRAM DIMM modules using Direct Mode (`0x8000`).
    pub fn apply_color(&mut self, color: RgbColor) -> Result<()> {
        for slot in &mut self.slots {
            if !slot.direct_initialized {
                ene_write_reg_byte(self.transport.as_mut(), slot.address, ENE_REG_DIRECT_SELECT, 0x01)?;
                ene_write_reg_byte(self.transport.as_mut(), slot.address, ENE_REG_MODE, 0x01)?;
                slot.direct_initialized = true;
            }

            let payload = encode_ene_rbg_buffer(color, slot.led_count);
            // ENE SMBus accepts 3-byte (1 LED) or up to 32-byte block chunks; 3-byte chunks
            // avoid SMBus block-length quirks across AMD PIIX4 and Intel I801 controllers.
            for (idx, chunk) in payload.chunks(3).enumerate() {
                let reg = ENE_REG_COLORS_DIRECT + (idx as u16) * 3;
                ene_write_reg_block(self.transport.as_mut(), slot.address, reg, chunk)?;
            }
            ene_write_reg_byte(self.transport.as_mut(), slot.address, ENE_REG_APPLY, 0x01)?;
        }
        Ok(())
    }
}

fn probe_ene_slot(transport: &mut dyn SmbusTransport, addr: u8) -> Option<EneDimmSlot> {
    let mut name_bytes = [0u8; 16];
    for (i, slot_byte) in name_bytes.iter_mut().enumerate() {
        let reg = ENE_REG_DEVICE_NAME + i as u16;
        match ene_read_reg_byte(transport, addr, reg) {
            Ok(b) => *slot_byte = b,
            Err(_) => return None,
        }
    }
    let name = String::from_utf8_lossy(&name_bytes)
        .trim_matches(|c: char| c == '\0' || !c.is_ascii_graphic())
        .to_string();

    if !is_valid_ene_dram_signature(&name) {
        debug!("SMBus 0x{addr:02X} returned non-ENE signature: '{name}'");
        return None;
    }

    let raw_leds = ene_read_reg_byte(transport, addr, ENE_REG_LED_COUNT)
        .ok()
        .map(usize::from)
        .unwrap_or(DEFAULT_TRIDENT_Z5_LEDS);
    let led_count = if (1..=MAX_ENE_LEDS).contains(&raw_leds) {
        raw_leds
    } else {
        DEFAULT_TRIDENT_Z5_LEDS
    };

    Some(EneDimmSlot {
        address: addr,
        device_name: name,
        led_count,
        direct_initialized: false,
    })
}

fn ene_read_reg_byte(transport: &mut dyn SmbusTransport, addr: u8, reg: u16) -> Result<u8> {
    transport.write_word_data(addr, ENE_CMD_REG_ADDR, encode_ene_register_word(reg))?;
    transport.read_byte_data(addr, ENE_CMD_READ_BYTE)
}

fn ene_write_reg_byte(transport: &mut dyn SmbusTransport, addr: u8, reg: u16, val: u8) -> Result<()> {
    transport.write_word_data(addr, ENE_CMD_REG_ADDR, encode_ene_register_word(reg))?;
    transport.write_byte_data(addr, ENE_CMD_WRITE_BYTE, val)
}

fn ene_write_reg_block(
    transport: &mut dyn SmbusTransport,
    addr: u8,
    reg: u16,
    data: &[u8],
) -> Result<()> {
    transport.write_word_data(addr, ENE_CMD_REG_ADDR, encode_ene_register_word(reg))?;
    if data.len() == 1 {
        transport.write_byte_data(addr, ENE_CMD_WRITE_BYTE, data[0])
    } else {
        transport.write_block_data(addr, ENE_CMD_WRITE_BLOCK, data)
    }
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
        fn select_slave(&mut self, addr: u8) -> Result<()> {
            let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), I2C_SLAVE, libc::c_ulong::from(addr)) };
            if ret < 0 {
                return Err(anyhow!(
                    "I2C_SLAVE(0x{addr:02X}) failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }

        fn smbus_xfer(
            &mut self,
            addr: u8,
            read_write: u8,
            command: u8,
            size: u32,
            data: &mut I2cSmbusData,
        ) -> Result<()> {
            self.select_slave(addr)?;
            let mut args = I2cSmbusIoctlData {
                read_write,
                command,
                size,
                data: data as *mut I2cSmbusData,
            };
            let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), I2C_SMBUS, &mut args) };
            if ret < 0 {
                return Err(anyhow!(
                    "I2C_SMBUS(0x{addr:02X}, cmd=0x{command:02X}) failed: {}",
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

        fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8> {
            let mut data = I2cSmbusData { byte: 0 };
            self.smbus_xfer(addr, I2C_SMBUS_READ, cmd, I2C_SMBUS_BYTE_DATA, &mut data)?;
            Ok(unsafe { data.byte })
        }

        fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()> {
            let mut data = I2cSmbusData { byte: val };
            self.smbus_xfer(addr, I2C_SMBUS_WRITE, cmd, I2C_SMBUS_BYTE_DATA, &mut data)
        }

        fn write_word_data(&mut self, addr: u8, cmd: u8, word: u16) -> Result<()> {
            let mut data = I2cSmbusData { word };
            self.smbus_xfer(addr, I2C_SMBUS_WRITE, cmd, I2C_SMBUS_WORD_DATA, &mut data)
        }

        fn write_block_data(&mut self, addr: u8, cmd: u8, payload: &[u8]) -> Result<()> {
            let len = payload.len().min(32);
            let mut block = [0u8; 34];
            block[0] = len as u8;
            block[1..=len].copy_from_slice(&payload[..len]);
            let mut data = I2cSmbusData { block };
            self.smbus_xfer(addr, I2C_SMBUS_WRITE, cmd, I2C_SMBUS_BLOCK_DATA, &mut data)
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
            let trimmed = name.trim();
            let lower = trimmed.to_ascii_lowercase();
            // Safety filter: ONLY motherboard SMBus controllers, never GPU DDC/CI adapters.
            let is_mobo_smbus = lower.contains("piix4")
                || lower.contains("i801")
                || lower.contains("smbus");
            let is_gpu = lower.contains("nvidia")
                || lower.contains("amdgpu")
                || lower.contains("radeon")
                || lower.contains("i915")
                || lower.contains("nouveau");
            if !is_mobo_smbus || is_gpu {
                continue;
            }
            let dev_path = format!("/dev/{fname}");
            if let Ok(file) = OpenOptions::new().read(true).write(true).open(&dev_path) {
                adapters.push(LinuxI2cSmbus {
                    file,
                    adapter: format!("{dev_path} ({trimmed})"),
                });
            }
        }
        adapters
    }
}

#[cfg(target_os = "windows")]
mod windows_pawnio {
    use super::*;
    use libloading::Library;
    use std::ffi::{c_char, c_void, CString};
    use std::path::PathBuf;

    const I2C_SMBUS_WRITE: u64 = 0;
    const I2C_SMBUS_READ: u64 = 1;
    const I2C_SMBUS_BYTE_DATA: u64 = 2;
    const I2C_SMBUS_WORD_DATA: u64 = 3;
    const I2C_SMBUS_BLOCK_DATA: u64 = 5;

    type FnPawnIoOpen = unsafe extern "system" fn(*mut *mut c_void) -> i32;
    type FnPawnIoLoad = unsafe extern "system" fn(*mut c_void, *const u8, usize) -> i32;
    type FnPawnIoExecute = unsafe extern "system" fn(
        *mut c_void,
        *const c_char,
        *const u64,
        usize,
        *mut u64,
        usize,
        *mut usize,
    ) -> i32;
    type FnPawnIoClose = unsafe extern "system" fn(*mut c_void) -> i32;

    pub struct PawnIoSmbusTransport {
        _lib: Library,
        handle: *mut c_void,
        exec_fn: FnPawnIoExecute,
        close_fn: FnPawnIoClose,
        adapter: String,
    }

    // SAFETY: Handle is owned exclusively by PawnIoSmbusTransport behind a Mutex in HardwareCollector.
    unsafe impl Send for PawnIoSmbusTransport {}

    impl Drop for PawnIoSmbusTransport {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    let _ = (self.close_fn)(self.handle);
                }
                self.handle = std::ptr::null_mut();
            }
        }
    }

    impl PawnIoSmbusTransport {
        pub fn open_first() -> Result<Self> {
            let dll_path = find_pawnio_dll()
                .ok_or_else(|| anyhow!("PawnIOLib.dll introuvable (C:\\Program Files\\PawnIO\\PawnIOLib.dll)"))?;

            let lib = unsafe { Library::new(&dll_path) }
                .map_err(|e| anyhow!("Chargement {} échoué: {e}", dll_path.display()))?;

            let open_fn: FnPawnIoOpen = unsafe { *lib.get(b"pawnio_open\0")? };
            let load_fn: FnPawnIoLoad = unsafe { *lib.get(b"pawnio_load\0")? };
            let exec_fn: FnPawnIoExecute = unsafe { *lib.get(b"pawnio_execute\0")? };
            let close_fn: FnPawnIoClose = unsafe { *lib.get(b"pawnio_close\0")? };

            let (module_name, blob) = load_smbus_module_blob().ok_or_else(|| {
                anyhow!("Module PawnIO SMBus introuvable (SmbusPIIX4.bin ou SmbusI801.bin)")
            })?;

            let mut handle: *mut c_void = std::ptr::null_mut();
            let hr = unsafe { open_fn(&mut handle) };
            if hr < 0 || handle.is_null() {
                return Err(anyhow!(
                    "pawnio_open refusé (HRESULT 0x{:08X} — droits administrateur/service requis)",
                    hr as u32
                ));
            }

            let hr_load = unsafe { load_fn(handle, blob.as_ptr(), blob.len()) };
            if hr_load < 0 {
                unsafe {
                    let _ = close_fn(handle);
                }
                return Err(anyhow!(
                    "pawnio_load({module_name}) échoué (HRESULT 0x{:08X})",
                    hr_load as u32
                ));
            }

            let mut transport = Self {
                _lib: lib,
                handle,
                exec_fn,
                close_fn,
                adapter: format!("PawnIO {module_name}"),
            };

            // On AMD PIIX4 (AM4 / AM5 B650), explicitly select primary SMBus port 0 (DIMM bus).
            if module_name.contains("PIIX4") {
                let _ = transport.exec_raw("ioctl_piix4_port_sel", &[0], 0);
            }

            Ok(transport)
        }

        fn exec_raw(&mut self, fn_name: &str, input: &[u64], out_len: usize) -> Result<Vec<u64>> {
            let c_name = CString::new(fn_name)?;
            let mut out = vec![0u64; out_len.max(1)];
            let mut ret_size: usize = 0;
            let hr = unsafe {
                (self.exec_fn)(
                    self.handle,
                    c_name.as_ptr(),
                    input.as_ptr(),
                    input.len(),
                    out.as_mut_ptr(),
                    out_len,
                    &mut ret_size,
                )
            };
            if hr < 0 {
                return Err(anyhow!(
                    "pawnio_execute({fn_name}) failed (HRESULT 0x{:08X})",
                    hr as u32
                ));
            }
            out.truncate(ret_size.min(out.len()));
            Ok(out)
        }

        fn xfer(
            &mut self,
            addr: u8,
            rw: u64,
            cmd: u8,
            proto: u64,
            data_words: &[u64],
        ) -> Result<Vec<u64>> {
            let mut input = [0u64; 9];
            input[0] = u64::from(addr);
            input[1] = rw;
            input[2] = u64::from(cmd);
            input[3] = proto;
            for (i, w) in data_words.iter().take(5).enumerate() {
                input[4 + i] = *w;
            }
            self.exec_raw("ioctl_smbus_xfer", &input, 5)
        }
    }

    impl SmbusTransport for PawnIoSmbusTransport {
        fn adapter_name(&self) -> &str {
            &self.adapter
        }

        fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8> {
            let out = self.xfer(addr, I2C_SMBUS_READ, cmd, I2C_SMBUS_BYTE_DATA, &[])?;
            Ok((out.first().copied().unwrap_or(0) & 0xFF) as u8)
        }

        fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()> {
            self.xfer(
                addr,
                I2C_SMBUS_WRITE,
                cmd,
                I2C_SMBUS_BYTE_DATA,
                &[u64::from(val)],
            )?;
            Ok(())
        }

        fn write_word_data(&mut self, addr: u8, cmd: u8, word: u16) -> Result<()> {
            self.xfer(
                addr,
                I2C_SMBUS_WRITE,
                cmd,
                I2C_SMBUS_WORD_DATA,
                &[u64::from(word)],
            )?;
            Ok(())
        }

        fn write_block_data(&mut self, addr: u8, cmd: u8, payload: &[u8]) -> Result<()> {
            let len = payload.len().min(32);
            let mut block = [0u8; 40];
            block[0] = len as u8;
            block[1..=len].copy_from_slice(&payload[..len]);
            let mut words = [0u64; 5];
            for (i, w) in words.iter_mut().enumerate() {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&block[i * 8..(i + 1) * 8]);
                *w = u64::from_le_bytes(bytes);
            }
            self.xfer(addr, I2C_SMBUS_WRITE, cmd, I2C_SMBUS_BLOCK_DATA, &words)?;
            Ok(())
        }
    }

    fn find_pawnio_dll() -> Option<PathBuf> {
        let mut candidates = vec![PathBuf::from(r"C:\Program Files\PawnIO\PawnIOLib.dll")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("PawnIOLib.dll"));
            }
        }
        candidates.into_iter().find(|p| p.exists())
    }

    fn load_smbus_module_blob() -> Option<(String, Vec<u8>)> {
        let names = ["SmbusPIIX4.bin", "SmbusI801.bin"];
        let mut dirs = vec![
            PathBuf::from(r"C:\Program Files\PawnIO\modules"),
            PathBuf::from(r"C:\Program Files\PawnIO"),
        ];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                dirs.push(dir.join("pawnio"));
                dirs.push(dir.to_path_buf());
            }
        }
        for name in names {
            for dir in &dirs {
                let p = dir.join(name);
                if let Ok(bytes) = std::fs::read(&p) {
                    if !bytes.is_empty() {
                        return Some((name.trim_end_matches(".bin").to_string(), bytes));
                    }
                }
            }
        }
        warn!("PawnIO DLL found, but SmbusPIIX4.bin / SmbusI801.bin module blobs were not found");
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockEneSmbus {
        last_reg: u16,
        writes: Vec<(u8, u16, Vec<u8>)>,
    }

    impl MockEneSmbus {
        fn new() -> Self {
            Self {
                last_reg: 0,
                writes: Vec::new(),
            }
        }
    }

    impl SmbusTransport for MockEneSmbus {
        fn adapter_name(&self) -> &str {
            "MockPIIX4"
        }

        fn read_byte_data(&mut self, addr: u8, cmd: u8) -> Result<u8> {
            assert_eq!(cmd, ENE_CMD_READ_BYTE);
            if addr != 0x71 && addr != 0x73 {
                return Err(anyhow!("NACK"));
            }
            let sig = b"DIMM_LED-0102\0\0\0";
            if (ENE_REG_DEVICE_NAME..ENE_REG_DEVICE_NAME + 16).contains(&self.last_reg) {
                return Ok(sig[(self.last_reg - ENE_REG_DEVICE_NAME) as usize]);
            }
            if self.last_reg == ENE_REG_LED_COUNT {
                return Ok(8);
            }
            Ok(0)
        }

        fn write_byte_data(&mut self, addr: u8, cmd: u8, val: u8) -> Result<()> {
            assert_eq!(cmd, ENE_CMD_WRITE_BYTE);
            self.writes.push((addr, self.last_reg, vec![val]));
            Ok(())
        }

        fn write_word_data(&mut self, _addr: u8, cmd: u8, word: u16) -> Result<()> {
            assert_eq!(cmd, ENE_CMD_REG_ADDR);
            self.last_reg = word.swap_bytes();
            Ok(())
        }

        fn write_block_data(&mut self, addr: u8, cmd: u8, data: &[u8]) -> Result<()> {
            assert_eq!(cmd, ENE_CMD_WRITE_BLOCK);
            self.writes.push((addr, self.last_reg, data.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn ene_register_and_rbg_encoding_match_trident_z5() {
        assert_eq!(encode_ene_register_word(0x8020), 0x2080);
        assert_eq!(encode_ene_register_word(0x80A0), 0xA080);

        let buf = encode_ene_rbg_buffer(RgbColor { r: 10, g: 20, b: 30 }, 8);
        assert_eq!(buf.len(), 24);
        assert_eq!(&buf[0..3], &[10, 30, 20]); // RBG byte order!
    }

    #[test]
    fn mock_trident_z5_dual_dimm_probe_and_apply() {
        let outcome = GskillEneSmbusBackend::probe_with_transport(Box::new(MockEneSmbus::new()), false);
        assert!(outcome.status.controllable);
        let mut backend = outcome.backend.expect("2 DIMMs discovered");
        assert_eq!(backend.slot_count(), 2);
        backend
            .apply_color(RgbColor { r: 255, g: 180, b: 90 })
            .expect("apply direct RGB");
    }
}
