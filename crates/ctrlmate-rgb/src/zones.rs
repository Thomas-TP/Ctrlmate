//! Zone model shared by the lighting engine and the hardware dispatcher.
//!
//! # Zone-id contract
//!
//! A *zone* is the unit exposed to Home Assistant / the UI as one `light` entity. A zone only exists
//! when a backend really drives it; the daemon must build its zone list from
//! [`crate::backends::RgbHardwareDispatcher::zones`] and never register zones from guesses
//! (USB vendor ids, device names, ...).
//!
//! | zone id              | driven by                                                                 |
//! |----------------------|---------------------------------------------------------------------------|
//! | `light_global`       | **virtual master** (not hardware): on/off + intensity multiplier, colour push |
//! | `light_motherboard`  | Gigabyte RGB Fusion 2 (ITE IT5702/IT8297) USB HID, or OpenRGB MOTHERBOARD device when no native driver |
//! | `light_ram`          | all ENE DRAM modules on the SMBus (native), or OpenRGB DRAM devices       |
//! | `light_fans`         | every NZXT RGB / Smart Device USB controller (one worker per controller)  |
//! | `light_cooler`       | OpenRGB COOLER devices (e.g. Kraken ring + logo)                          |
//! | `light_gpu`          | OpenRGB GPU devices (absent for GPUs without controllable RGB)             |
//! | `light_openrgb_<n>`  | any other OpenRGB device that no native backend drives                    |
//!
//! A zone listed with `controllable == false` is *informational only* (for example the Samsung 990 PRO
//! heatsink LED, or DRAM whose SMBus path is locked by the anti-cheat-safe mode). Those must not be
//! registered in the lighting engine and never receive frames. `reason` is a stable snake_case code
//! (never a sentence): `None` when controllable.

use serde::{Deserialize, Serialize};

/// Stable id of the virtual master zone.
pub const ZONE_GLOBAL: &str = "light_global";
pub const ZONE_MOTHERBOARD: &str = "light_motherboard";
pub const ZONE_RAM: &str = "light_ram";
pub const ZONE_FANS: &str = "light_fans";
pub const ZONE_COOLER: &str = "light_cooler";
pub const ZONE_GPU: &str = "light_gpu";
pub const ZONE_STORAGE: &str = "light_storage";

/// What a zone accepts. All RGB backends are colour-only devices, so Kelvin and intensity are
/// derived in software by the engine; the flags still let a future white-only backend opt out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneCapabilities {
    pub rgb: bool,
    pub kelvin: bool,
    pub intensity: bool,
}

impl ZoneCapabilities {
    pub const FULL: Self = Self {
        rgb: true,
        kelvin: true,
        intensity: true,
    };
    pub const NONE: Self = Self {
        rgb: false,
        kelvin: false,
        intensity: false,
    };
}

/// One real, discovered lighting zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneInfo {
    /// Stable zone id, see the module documentation.
    pub id: String,
    /// Human readable device label (device names are data, not UI sentences).
    pub name: String,
    /// Backend identifier: `gigabyte_ite`, `nzxt_rgb`, `gskill_ene_smbus`, `openrgb`, `samsung_nvme_led`.
    pub backend: String,
    /// Transport description, e.g. `USB HID 048D:5702`.
    pub transport: String,
    /// Number of LEDs this zone drives (0 when unknown / not controllable).
    pub led_count: u32,
    pub capabilities: ZoneCapabilities,
    pub controllable: bool,
    /// Stable code explaining why the zone is not controllable (`None` when it is).
    pub reason: Option<String>,
}

impl ZoneInfo {
    pub fn controllable(
        id: &str,
        name: impl Into<String>,
        backend: &str,
        transport: impl Into<String>,
        led_count: u32,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.into(),
            backend: backend.to_string(),
            transport: transport.into(),
            led_count,
            capabilities: ZoneCapabilities::FULL,
            controllable: true,
            reason: None,
        }
    }

    pub fn uncontrollable(
        id: &str,
        name: impl Into<String>,
        backend: &str,
        transport: impl Into<String>,
        reason: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.into(),
            backend: backend.to_string(),
            transport: transport.into(),
            led_count: 0,
            capabilities: ZoneCapabilities::NONE,
            controllable: false,
            reason: Some(reason.to_string()),
        }
    }
}
