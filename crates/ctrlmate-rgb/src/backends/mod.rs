//! Unified hardware RGB backend dispatcher:
//! - Native USB HID: Gigabyte ITE (`048D:5702`) + NZXT RGB Hubs (`1E71:2012`)
//! - Native SMBus: G.Skill Trident Z5 / ENE DRAM (`0x70..=0x77` via Linux `i2c-dev` or Windows opt-in PawnIO)
//! - Safe NVMe Diagnostics: Samsung 990 PRO Heatsink LED detection & status
//! - Interop Fallback: OpenRGB SDK TCP client (`127.0.0.1:6742`)

pub mod gigabyte_ite;
pub mod gskill_ene_smbus;
pub mod nzxt_rgb;
pub mod openrgb;
pub mod samsung_nvme_led;

use ctrlmate_protocol::{RgbColor, RgbDeviceStatus};
use hidapi::HidApi;
use tracing::{info, warn};

/// Active physical RGB backends discovered on the host.
pub struct RgbHardwareDispatcher {
    gigabyte: Option<gigabyte_ite::GigabyteIteBackend>,
    nzxt: Vec<nzxt_rgb::NzxtRgbBackend>,
    gskill_ene: Option<gskill_ene_smbus::GskillEneSmbusBackend>,
    openrgb: Option<openrgb::OpenRgbClient>,
    device_statuses: Vec<RgbDeviceStatus>,
}

impl RgbHardwareDispatcher {
    pub fn discover(allow_kernel_smbus: bool) -> Self {
        let (gigabyte, nzxt) = match HidApi::new() {
            Ok(api) => (
                gigabyte_ite::GigabyteIteBackend::probe(&api),
                nzxt_rgb::NzxtRgbBackend::probe(&api),
            ),
            Err(err) => {
                warn!("HID API unavailable for RGB discovery: {err}");
                (None, Vec::new())
            }
        };

        let openrgb = openrgb::OpenRgbClient::try_connect(6742);
        let has_openrgb_ram = openrgb
            .as_ref()
            .is_some_and(|c| c.controllers().iter().any(|d| d.device_type == 1));

        let gskill_outcome =
            gskill_ene_smbus::GskillEneSmbusBackend::probe(allow_kernel_smbus, has_openrgb_ram);
        let samsung_statuses = samsung_nvme_led::detect_samsung_nvme_led_status();

        let mut device_statuses = Vec::new();
        if let Some(gb) = &gigabyte {
            device_statuses.push(RgbDeviceStatus {
                id: "gigabyte_ite".into(),
                name: gb.name(),
                zone_id: "light_motherboard".into(),
                transport: "USB HID (048D:5702)".into(),
                controllable: true,
                reason: "Contrôle natif direct actif (Zero-Kernel).".into(),
            });
        }
        for (idx, n) in nzxt.iter().enumerate() {
            device_statuses.push(RgbDeviceStatus {
                id: format!("nzxt_rgb_{idx}"),
                name: n.name(),
                zone_id: "light_cooler".into(),
                transport: "USB HID (1E71:2012)".into(),
                controllable: true,
                reason: "Contrôle natif direct actif (Zero-Kernel).".into(),
            });
        }
        device_statuses.push(gskill_outcome.status);
        device_statuses.extend(samsung_statuses);

        let dispatcher = Self {
            gigabyte,
            nzxt,
            gskill_ene: gskill_outcome.backend,
            openrgb,
            device_statuses,
        };
        info!(
            "RGB hardware backends ready: {:?}",
            dispatcher.active_backend_names()
        );
        dispatcher
    }

    pub fn active_backend_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(gb) = &self.gigabyte {
            names.push(gb.name());
        }
        for n in &self.nzxt {
            names.push(n.name());
        }
        if let Some(ram) = &self.gskill_ene {
            names.push(ram.name());
        }
        if let Some(orgb) = &self.openrgb {
            names.push(format!(
                "OpenRGB SDK ({} device(s))",
                orgb.controllers().len()
            ));
        }
        names
    }

    pub fn device_statuses(&self) -> &[RgbDeviceStatus] {
        &self.device_statuses
    }

    pub fn has_ram_rgb(&self) -> bool {
        self.gskill_ene.is_some()
            || self
                .openrgb
                .as_ref()
                .is_some_and(|c| c.controllers().iter().any(|d| d.device_type == 1))
    }

    /// Pushes a rendered RGB frame to the physical controller(s) mapped to `zone_entity_id`.
    pub fn push_zone_frame(&mut self, zone_entity_id: &str, color: RgbColor) {
        match zone_entity_id {
            "light_motherboard" => {
                if let Some(gb) = &mut self.gigabyte {
                    if let Err(err) = gb.apply_color(color) {
                        warn!("Gigabyte IT5702 write error: {err}");
                    }
                }
            }
            "light_cooler" | "light_fans" => {
                for n in &mut self.nzxt {
                    if let Err(err) = n.apply_color(color) {
                        warn!("NZXT RGB write error: {err}");
                    }
                }
            }
            "light_ram" => {
                if let Some(ram) = &mut self.gskill_ene {
                    if let Err(err) = ram.apply_color(color) {
                        warn!("G.Skill ENE SMBus write error: {err}");
                    }
                } else if let Some(orgb) = &mut self.openrgb {
                    if let Err(err) = orgb.apply_color(color, true) {
                        warn!("OpenRGB RAM write error: {err}");
                    }
                }
            }
            "light_global" => {
                // Global updates already cascade to registered zones via LightingEngine;
                // also push to any OpenRGB devices not individually zoned.
                if let Some(orgb) = &mut self.openrgb {
                    if let Err(err) = orgb.apply_color(color, false) {
                        warn!("OpenRGB global write error: {err}");
                    }
                }
            }
            _ => {}
        }
    }
}
