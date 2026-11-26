//! Hardware discovery and frame dispatch.
//!
//! * Native USB HID: Gigabyte RGB Fusion 2 (`048D:5702`) and NZXT RGB controllers (`1E71:2012`, ...)
//! * Native SMBus: G.Skill Trident Z5 / ENE DRAM (Linux i2c-dev, Windows PawnIO in deep-hardware mode)
//! * Read-only diagnostics: Samsung 990 PRO Heatsink LED (never controllable)
//! * OpenRGB SDK client for everything else (e.g. Kraken ring/logo, GPU RGB), deduplicated against
//!   the native backends so a device is never written by two drivers.
//!
//! [`RgbHardwareDispatcher::discover`] returns the **real** zones ([`RgbHardwareDispatcher::zones`]);
//! [`RgbHardwareDispatcher::submit_frame`] is non-blocking: each backend runs on its own worker
//! thread with a latest-frame-wins slot and a per-backend maximum rate (see [`crate::pipeline`]).

pub mod gigabyte_ite;
pub mod gskill_ene_smbus;
pub mod nzxt_rgb;
pub mod openrgb;
pub mod samsung_nvme_led;

pub use crate::pipeline::{BackendStats, LedSink};

use crate::pipeline::BackendWorker;
use crate::zones::{ZONE_COOLER, ZONE_FANS, ZONE_GPU, ZONE_MOTHERBOARD, ZONE_RAM, ZoneInfo};
use ctrlmate_protocol::{RgbColor, RgbDeviceStatus};
use hidapi::HidApi;
use openrgb::{NativeCoverage, OpenRgbClient, OpenRgbDeviceSink, device_type};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// Discovery options.
#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    /// Allow the SMBus path that needs a kernel driver on Windows (PawnIO). Must be `false` while
    /// an anti-cheat is running (architecture §7).
    pub allow_kernel_smbus: bool,
    /// Talk to a locally running OpenRGB SDK server.
    pub enable_openrgb: bool,
    pub openrgb_port: u16,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            allow_kernel_smbus: false,
            enable_openrgb: true,
            openrgb_port: 6742,
        }
    }
}

/// Active physical RGB backends discovered on the host.
#[derive(Default)]
pub struct RgbHardwareDispatcher {
    workers: Vec<BackendWorker>,
    zone_workers: BTreeMap<String, Vec<usize>>,
    zones: Vec<ZoneInfo>,
    device_statuses: Vec<RgbDeviceStatus>,
}

/// Merges several sinks of one zone into one [`ZoneInfo`].
fn merge_zone(zones: &mut BTreeMap<String, ZoneInfo>, z: ZoneInfo) {
    match zones.get_mut(&z.id) {
        Some(existing) if existing.controllable && z.controllable => {
            existing.led_count += z.led_count;
            existing.name = format!("{} + {}", existing.name, z.name);
        }
        Some(existing) if !existing.controllable && z.controllable => *existing = z,
        Some(_) => {}
        None => {
            zones.insert(z.id.clone(), z);
        }
    }
}

impl RgbHardwareDispatcher {
    /// Empty dispatcher (tests, or when lighting is disabled).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Source-compatible entry point: OpenRGB enabled, default port.
    pub fn discover(allow_kernel_smbus: bool) -> Self {
        Self::discover_with(&DiscoverOptions {
            allow_kernel_smbus,
            ..DiscoverOptions::default()
        })
    }

    pub fn discover_with(opts: &DiscoverOptions) -> Self {
        let mut d = Self::empty();
        let mut zone_map: BTreeMap<String, ZoneInfo> = BTreeMap::new();

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

        let openrgb = if opts.enable_openrgb {
            OpenRgbClient::try_connect(opts.openrgb_port)
        } else {
            None
        };
        let has_openrgb_ram = openrgb.as_ref().is_some_and(|c| {
            c.devices()
                .iter()
                .any(|dev| dev.device_type == device_type::DRAM)
        });
        let ene = gskill_ene_smbus::GskillEneSmbusBackend::probe(
            opts.allow_kernel_smbus,
            has_openrgb_ram,
        );

        let mut native = NativeCoverage::default();

        // --- Gigabyte
        if let Some(gb) = gigabyte {
            native.gigabyte_fusion2 = true;
            let name = gb.name();
            let zone = ZoneInfo::controllable(
                ZONE_MOTHERBOARD,
                name.clone(),
                "gigabyte_ite",
                format!("USB HID 048D:{:04X}", gb.pid()),
                0,
            );
            d.device_statuses.push(RgbDeviceStatus {
                id: "gigabyte_ite".into(),
                name,
                zone_id: ZONE_MOTHERBOARD.into(),
                transport: zone.transport.clone(),
                controllable: true,
                reason: "ok".into(),
            });
            merge_zone(&mut zone_map, zone.clone());
            d.add_sink(ZONE_MOTHERBOARD, Box::new(gb));
        }

        // --- NZXT hubs
        for (idx, n) in nzxt.into_iter().enumerate() {
            let leds = n.total_leds();
            let id = format!("nzxt_rgb_{idx}");
            let transport = format!("USB HID 1E71:{:04X}", n.pid());
            if leds == 0 {
                d.device_statuses.push(RgbDeviceStatus {
                    id,
                    name: n.name(),
                    zone_id: ZONE_FANS.into(),
                    transport,
                    controllable: false,
                    reason: "no_accessories_detected".into(),
                });
                continue;
            }
            native.nzxt_hubs = true;
            d.device_statuses.push(RgbDeviceStatus {
                id,
                name: n.name(),
                zone_id: ZONE_FANS.into(),
                transport: transport.clone(),
                controllable: true,
                reason: "ok".into(),
            });
            merge_zone(
                &mut zone_map,
                ZoneInfo::controllable(ZONE_FANS, n.name(), "nzxt_rgb", transport, leds as u32),
            );
            d.add_sink(ZONE_FANS, Box::new(n));
        }

        // --- ENE DRAM
        d.device_statuses.push(ene.status);
        match ene.backend {
            Some(be) => {
                native.ene_dram = true;
                merge_zone(&mut zone_map, ene.zone.clone());
                d.add_sink(ZONE_RAM, Box::new(be));
            }
            None if !ene.zone.controllable => merge_zone(&mut zone_map, ene.zone),
            None => {} // covered by OpenRGB below
        }

        // --- Samsung (informational only)
        d.device_statuses
            .extend(samsung_nvme_led::detect_samsung_nvme_led_status());
        for z in samsung_nvme_led::samsung_zones() {
            merge_zone(&mut zone_map, z);
        }

        // --- OpenRGB (after native coverage is known)
        if let Some(client) = openrgb {
            let devices: Vec<_> = client.devices().to_vec();
            let shared = Arc::new(Mutex::new(client));
            for dev in devices {
                let status_id = format!("openrgb_{}", dev.index);
                let zone_id = zone_for_openrgb(&dev);
                if let Some(why) = openrgb::natively_driven(&dev, native) {
                    d.device_statuses.push(RgbDeviceStatus {
                        id: status_id,
                        name: dev.name.clone(),
                        zone_id,
                        transport: "OpenRGB SDK".into(),
                        controllable: false,
                        reason: why.into(),
                    });
                    continue;
                }
                if dev.led_count == 0 {
                    continue;
                }
                d.device_statuses.push(RgbDeviceStatus {
                    id: status_id,
                    name: dev.name.clone(),
                    zone_id: zone_id.clone(),
                    transport: "OpenRGB SDK".into(),
                    controllable: true,
                    reason: "ok".into(),
                });
                merge_zone(
                    &mut zone_map,
                    ZoneInfo::controllable(
                        &zone_id,
                        dev.name.clone(),
                        "openrgb",
                        "OpenRGB SDK",
                        dev.led_count,
                    ),
                );
                d.add_sink(
                    &zone_id,
                    Box::new(OpenRgbDeviceSink::new(shared.clone(), &dev)),
                );
            }
        }

        d.zones = zone_map.into_values().collect();
        info!(
            "RGB discovery: {} zone(s), {} backend worker(s): {:?}",
            d.zones.len(),
            d.workers.len(),
            d.active_backend_names()
        );
        d
    }

    /// Registers a sink for `zone_id` and starts its worker thread.
    pub fn add_sink(&mut self, zone_id: &str, sink: Box<dyn LedSink>) {
        let idx = self.workers.len();
        self.workers.push(BackendWorker::spawn(sink));
        self.zone_workers
            .entry(zone_id.to_string())
            .or_default()
            .push(idx);
    }

    /// Adds a zone description (used by [`Self::add_sink`] callers that build dispatchers manually).
    pub fn add_zone(&mut self, zone: ZoneInfo) {
        self.zones.retain(|z| z.id != zone.id);
        self.zones.push(zone);
    }

    /// Every discovered zone, including informational (non-controllable) ones.
    pub fn zones(&self) -> &[ZoneInfo] {
        &self.zones
    }

    /// Only the zones that can receive frames.
    pub fn controllable_zones(&self) -> Vec<ZoneInfo> {
        self.zones
            .iter()
            .filter(|z| z.controllable)
            .cloned()
            .collect()
    }

    pub fn active_backend_names(&self) -> Vec<String> {
        self.workers.iter().map(|w| w.name().to_string()).collect()
    }

    pub fn device_statuses(&self) -> &[RgbDeviceStatus] {
        &self.device_statuses
    }

    pub fn has_ram_rgb(&self) -> bool {
        self.zones
            .iter()
            .any(|z| z.id == ZONE_RAM && z.controllable)
    }

    /// Per-backend counters (submitted / written / coalesced / failed, last error).
    pub fn backend_stats(&self) -> Vec<BackendStats> {
        self.workers.iter().map(BackendWorker::stats).collect()
    }

    /// Non-blocking: hands `color` to every backend of `zone_id` (latest frame wins).
    /// Returns `false` when no backend drives that zone (including the virtual `light_global`).
    pub fn submit_frame(&self, zone_id: &str, color: RgbColor) -> bool {
        let Some(idxs) = self.zone_workers.get(zone_id) else {
            return false;
        };
        for &i in idxs {
            self.workers[i].submit(color);
        }
        !idxs.is_empty()
    }

    /// Source-compatible shim for the previous blocking API; now non-blocking.
    pub fn push_zone_frame(&mut self, zone_entity_id: &str, color: RgbColor) {
        self.submit_frame(zone_entity_id, color);
    }
}

fn zone_for_openrgb(dev: &openrgb::OpenRgbDevice) -> String {
    match dev.device_type {
        device_type::MOTHERBOARD => ZONE_MOTHERBOARD.to_string(),
        device_type::DRAM => ZONE_RAM.to_string(),
        device_type::GPU => ZONE_GPU.to_string(),
        device_type::COOLER => ZONE_COOLER.to_string(),
        _ => format!("light_openrgb_{}", dev.index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Counting(Arc<AtomicUsize>, Arc<Mutex<Option<RgbColor>>>);
    impl LedSink for Counting {
        fn name(&self) -> String {
            "counting".into()
        }
        fn write(&mut self, c: RgbColor) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            *self.1.lock().expect("lock") = Some(c);
            Ok(())
        }
        fn min_interval(&self) -> Duration {
            Duration::from_millis(1)
        }
    }

    #[test]
    fn frames_fan_out_per_zone_and_unknown_zones_are_rejected() {
        let n = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(Mutex::new(None));
        let mut d = RgbHardwareDispatcher::empty();
        d.add_sink("light_fans", Box::new(Counting(n.clone(), last.clone())));
        d.add_sink("light_fans", Box::new(Counting(n.clone(), last.clone())));
        d.add_zone(ZoneInfo::controllable("light_fans", "fans", "t", "usb", 16));
        d.add_zone(ZoneInfo::uncontrollable(
            "light_storage",
            "ssd",
            "samsung",
            "nvme",
            "no_public_protocol",
        ));
        let c = RgbColor { r: 1, g: 2, b: 3 };
        assert!(d.submit_frame("light_fans", c));
        assert!(!d.submit_frame("light_global", c));
        assert!(!d.submit_frame("light_gpu", c));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(n.load(Ordering::SeqCst), 2);
        assert_eq!(*last.lock().expect("lock"), Some(c));
        assert_eq!(d.controllable_zones().len(), 1);
        assert_eq!(d.zones().len(), 2);
        assert!(!d.has_ram_rgb());
        assert_eq!(d.backend_stats().len(), 2);
    }

    #[test]
    fn merging_zones_sums_leds() {
        let mut m = BTreeMap::new();
        merge_zone(
            &mut m,
            ZoneInfo::controllable("light_fans", "A", "x", "u", 8),
        );
        merge_zone(
            &mut m,
            ZoneInfo::controllable("light_fans", "B", "x", "u", 16),
        );
        assert_eq!(m["light_fans"].led_count, 24);
        merge_zone(
            &mut m,
            ZoneInfo::uncontrollable("light_ram", "r", "x", "u", "requires_deep_hardware"),
        );
        merge_zone(
            &mut m,
            ZoneInfo::controllable("light_ram", "r2", "x", "u", 4),
        );
        assert!(m["light_ram"].controllable);
    }
}
