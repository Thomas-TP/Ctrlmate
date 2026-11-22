//! Unified hardware RGB backend dispatcher (Gigabyte ITE HID + NZXT RGB HID + OpenRGB SDK).

pub mod gigabyte_ite;
pub mod nzxt_rgb;
pub mod openrgb;

use ctrlmate_protocol::RgbColor;
use hidapi::HidApi;
use tracing::{info, warn};

/// Active physical RGB backends discovered on the host.
pub struct RgbHardwareDispatcher {
    gigabyte: Option<gigabyte_ite::GigabyteIteBackend>,
    nzxt: Vec<nzxt_rgb::NzxtRgbBackend>,
    openrgb: Option<openrgb::OpenRgbClient>,
}

impl RgbHardwareDispatcher {
    pub fn discover() -> Self {
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

        let dispatcher = Self {
            gigabyte,
            nzxt,
            openrgb,
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
        if let Some(orgb) = &self.openrgb {
            names.push(format!(
                "OpenRGB SDK ({} device(s))",
                orgb.controllers().len()
            ));
        }
        names
    }

    pub fn has_openrgb_ram(&self) -> bool {
        self.openrgb
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
                if let Some(orgb) = &mut self.openrgb {
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
