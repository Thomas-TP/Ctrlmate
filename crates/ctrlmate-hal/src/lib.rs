//! Hardware layer of `ctrlmated`: honest telemetry, the cooling runtime, lighting and the
//! interactive-session module shared with `ctrlmate-agent`.
//!
//! Layout:
//! * [`HardwareCollector`]: manifest + sensor poll (blocking, called from the daemon's hardware thread).
//! * [`thermal`]: the cooling thread (own 1 s timer, safety watchdog, hardware restore on shutdown).
//! * [`lighting`]: [`lighting::LightingHub`], independent of the sensor side.
//! * [`platform`] + [`anticheat`]: anti-cheat gate, bridge lifecycle, RGB discovery.
//! * [`session`]: everything bound to the interactive session (agent / in-process fallback).
//! * [`updates`]: OS updates (WUA / PackageKit).
//!
//! Honest sensors: every telemetry value is optional. A missing sensor produces no state at all (the
//! daemon marks it unavailable and explains why in `sensor_status`); nothing is ever defaulted.

pub mod anticheat;
pub mod conflicts;
pub mod gpu;
pub mod hid;
pub mod kraken;
pub mod lhm_bridge;
pub mod lighting;
pub mod os_control;
pub mod platform;
pub mod session;
pub mod smart_poll;
pub mod storage;
pub mod system;
pub mod thermal;
pub mod updates;
#[cfg(target_os = "linux")]
mod updates_pk;

pub use ctrlmate_cooling::sanitize_thresholds;

use chrono::Utc;
use ctrlmate_protocol::{
    AllowlistedScript, ChildDeviceInfo, CoolingPolicy, CoolingPreset, DetectedHidDevice,
    DeviceCategory, DeviceManifest, EntityCategory, EntityDescriptor, EntityPlatform, EntityState,
    EntityValue, LightCapabilities, NumberRange, OsUpdateState, PROTOCOL_VERSION, RgbDeviceStatus,
    SecurityMode, SoftwareConflict, StateClass, ThermalThresholds, UpdatesInfo,
};
use ctrlmate_rgb::backends::DiscoverOptions;
use ctrlmate_rgb::zones::{
    ZONE_COOLER, ZONE_FANS, ZONE_GLOBAL, ZONE_GPU, ZONE_MOTHERBOARD, ZONE_RAM,
};
use lhm_bridge::{DeepSensorSnapshot, LhmBridgeManager};
use lighting::LightingHub;
use platform::{Platform, Supervisor};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thermal::{CoolingRuntime, KrakenReading, RealThermalIo, Telemetry};
use tracing::info;

/// Construction options of the collector.
#[derive(Debug, Clone)]
pub struct CollectorOptions {
    pub security_mode: SecurityMode,
    /// Initial watchdog thresholds (clamped by the cooling engine).
    pub thermal: ThermalThresholds,
    /// LibreHardwareMonitor control ids declared as chassis fans (`--chassis-id` of the bridge).
    pub chassis_fan_ids: Vec<String>,
    pub enable_openrgb: bool,
    pub openrgb_port: u16,
    /// Period of the cooling watchdog thread.
    pub watchdog_period: Duration,
}

impl Default for CollectorOptions {
    fn default() -> Self {
        Self {
            security_mode: SecurityMode::ZeroKernelSafe,
            thermal: ThermalThresholds::default(),
            chassis_fan_ids: Vec::new(),
            enable_openrgb: true,
            openrgb_port: 6742,
            watchdog_period: thermal::TICK,
        }
    }
}

/// The hardware features the manifest depends on. A change means the manifest must be rebuilt.
/// Sensor-ish parts only ever grow (an entity that was registered stays registered and turns
/// `unavailable` when its source is paused), the rest tracks the current hardware.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Caps {
    // sticky
    cpu_temp: bool,
    cpu_power: bool,
    ram_temp: bool,
    vrm: bool,
    chipset: bool,
    board_name: Option<String>,
    ccds: BTreeMap<String, String>,
    fans: BTreeMap<String, String>,
    storage_temp: BTreeSet<String>,
    // current
    gpus: usize,
    kraken: Option<bool>,
    zones: BTreeSet<String>,
    storage: BTreeSet<String>,
}

impl Caps {
    /// Merges `now` into `self`; `true` when anything changed.
    fn absorb(&mut self, now: &Caps) -> bool {
        let before = self.clone();
        self.cpu_temp |= now.cpu_temp;
        self.cpu_power |= now.cpu_power;
        self.ram_temp |= now.ram_temp;
        self.vrm |= now.vrm;
        self.chipset |= now.chipset;
        if now.board_name.is_some() {
            self.board_name.clone_from(&now.board_name);
        }
        self.ccds.extend(now.ccds.clone());
        self.fans.extend(now.fans.clone());
        self.storage_temp.extend(now.storage_temp.iter().cloned());
        self.gpus = now.gpus;
        self.kraken = now.kraken;
        self.zones.clone_from(&now.zones);
        self.storage.clone_from(&now.storage);
        *self != before
    }
}

/// `/lpc/it8688e/0/fan/1` -> `it8688e_1` (stable across runs; never derived from RPM or order).
pub fn fan_entity_suffix(bridge_id: &str) -> String {
    let parts: Vec<&str> = bridge_id.split('/').filter(|p| !p.is_empty()).collect();
    if let Some(pos) = parts.iter().position(|p| p.eq_ignore_ascii_case("fan"))
        && pos >= 2
        && let Some(n) = parts.get(pos + 1)
    {
        let chip = parts[1];
        return sanitize(&format!("{chip}_{n}"));
    }
    sanitize(bridge_id)
}

/// `CCD1 (Tdie)` -> `ccd1`.
fn ccd_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if let Some(i) = lower.find("ccd") {
        let digits: String = lower[i + 3..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !digits.is_empty() {
            return format!("ccd{digits}");
        }
    }
    sanitize(name)
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn caps_from(
    tel: Option<&Telemetry>,
    zones: &[ctrlmate_rgb::ZoneInfo],
    gpus: usize,
    kraken: Option<bool>,
    storage: &[(String, bool)],
) -> Caps {
    let mut c = Caps {
        gpus,
        kraken,
        zones: zones
            .iter()
            .filter(|z| z.controllable)
            .map(|z| z.id.clone())
            .collect(),
        storage: storage.iter().map(|(id, _)| id.clone()).collect(),
        storage_temp: storage
            .iter()
            .filter(|(_, t)| *t)
            .map(|(id, _)| id.clone())
            .collect(),
        ..Caps::default()
    };
    if let Some(t) = tel {
        c.cpu_temp =
            t.cpu.package_c.is_some() || t.deep.as_ref().is_some_and(|d| d.cpu_temp_c.is_some());
        if let Some(d) = &t.deep {
            c.cpu_power = d.cpu_power_w.is_some();
            c.ram_temp = d.ram_temp_c.is_some();
            c.vrm = d.motherboard_vrm_temp_c.is_some();
            c.chipset = d.motherboard_chipset_temp_c.is_some();
            c.board_name.clone_from(&d.motherboard_name);
            for ccd in &d.cpu_ccd_temps_c {
                c.ccds.insert(ccd_key(&ccd.name), ccd.name.clone());
            }
            for f in &d.case_fans {
                c.fans.insert(fan_entity_suffix(&f.id), f.name.clone());
            }
        }
        for (name, _) in &t.cpu.ccd_c {
            c.ccds.insert(ccd_key(name), name.clone());
        }
    }
    c
}

/// Central hardware & telemetry collector running inside `ctrlmated`.
pub struct HardwareCollector {
    device_id: String,
    sw_version: String,
    system: system::SystemProvider,
    storage: storage::StorageProvider,
    hid_devices: Vec<DetectedHidDevice>,
    gpu_descriptors: Vec<gpu::GpuDescriptor>,
    kraken_info: Option<(String, bool)>,
    lighting: LightingHub,
    cooling: Arc<CoolingRuntime>,
    platform: Arc<Platform>,
    supervisor: Supervisor,
    os_updates: OsUpdateState,
    /// Capabilities the last manifest was built for (grows monotonically between rescans).
    seen: Caps,
}

impl HardwareCollector {
    pub fn new(device_id: String, sw_version: String, security_mode: SecurityMode) -> Self {
        Self::with_options(
            device_id,
            sw_version,
            CollectorOptions {
                security_mode,
                ..CollectorOptions::default()
            },
        )
    }

    pub fn with_options(device_id: String, sw_version: String, opts: CollectorOptions) -> Self {
        let system = system::SystemProvider::new();
        let storage = storage::StorageProvider::new();
        let hid_devices = hid::scan_known_hid_devices();
        let gpu = gpu::GpuProvider::new();
        let gpu_descriptors = gpu.descriptors().to_vec();
        let kraken = kraken::KrakenDriver::probe();
        let kraken_info = kraken
            .as_ref()
            .map(|k| (k.model_name().to_string(), k.has_fan_channel()));

        let bridge = Arc::new(LhmBridgeManager::new(opts.chassis_fan_ids.clone()));
        let lighting = LightingHub::new();
        let cooling = Arc::new(CoolingRuntime::spawn(
            Box::new(RealThermalIo {
                kraken,
                gpu,
                bridge: Arc::clone(&bridge),
            }),
            opts.thermal,
            opts.watchdog_period,
        ));
        let platform = Arc::new(Platform::new(
            bridge,
            lighting.clone(),
            Arc::clone(&cooling),
            opts.security_mode,
            DiscoverOptions {
                allow_kernel_smbus: false,
                enable_openrgb: opts.enable_openrgb,
                openrgb_port: opts.openrgb_port,
            },
        ));
        // Know about anti-cheats before anything kernel-adjacent starts.
        platform.scan(Instant::now());
        platform.reconcile(true);
        let supervisor = Supervisor::spawn(Arc::clone(&platform));

        info!(
            "HardwareCollector initialized (GPU: {}, Kraken: {}, USB HID: {}, RGB backends: {:?}, bridge: {}, anti-cheat paused: {})",
            gpu_descriptors.len(),
            kraken_info.is_some(),
            hid_devices.len(),
            lighting.backend_names(),
            platform.bridge.is_running(),
            platform.anti_cheat_paused()
        );

        Self {
            device_id,
            sw_version,
            system,
            storage,
            hid_devices,
            gpu_descriptors,
            kraken_info,
            lighting,
            cooling,
            platform,
            supervisor,
            os_updates: OsUpdateState {
                installed_version: system::os_version_string(),
                latest_version: system::os_version_string(),
                pending_count: 0,
                pending_titles: Vec::new(),
                in_progress: false,
                reboot_required: updates::is_reboot_required(),
                last_checked: None,
            },
            seen: Caps::default(),
        }
    }

    /// Re-probes USB HID controllers, the Kraken, ENE SMBus RAM and OpenRGB. Use it right after closing
    /// a conflicting vendor application (CAM, Armoury Crate, ...).
    pub fn rescan_hardware(&mut self) {
        self.hid_devices = hid::scan_known_hid_devices();
        // Release the old Kraken handle first (this also writes its default curves back).
        self.cooling.replace_kraken(None);
        let k = kraken::KrakenDriver::probe();
        self.kraken_info = k
            .as_ref()
            .map(|k| (k.model_name().to_string(), k.has_fan_channel()));
        self.cooling.replace_kraken(k);
        self.cooling.invalidate();
        self.platform.reconcile(true);
        self.seen = Caps::default();
        info!(
            "Hardware rescanned (Kraken: {}, RGB backends: {:?})",
            self.kraken_info.is_some(),
            self.lighting.backend_names()
        );
    }

    pub fn security_mode(&self) -> SecurityMode {
        self.platform.mode()
    }

    /// Switches the security mode: the bridge and the SMBus path open / close immediately (and stay
    /// closed while an anti-cheat runs).
    pub fn set_security_mode(&mut self, mode: SecurityMode) {
        self.platform.set_mode(mode);
    }

    pub fn deep_bridge_active(&self) -> bool {
        self.platform.bridge.is_running()
    }

    /// An anti-cheat is running (or was within the last 30 s): deep hardware is paused.
    pub fn anti_cheat_paused(&self) -> bool {
        self.platform.anti_cheat_paused()
    }

    pub fn rgb_backend_names(&self) -> Vec<String> {
        self.lighting.backend_names()
    }

    /// Stable status codes per RGB device (`ok`, `requires_deep_hardware`, ...).
    pub fn rgb_device_statuses(&self) -> Vec<RgbDeviceStatus> {
        self.lighting.device_statuses()
    }

    pub fn hid_devices(&self) -> &[DetectedHidDevice] {
        &self.hid_devices
    }

    /// Lighting handle; clone it for a dedicated lighting thread (it shares no lock with sensor polling).
    pub fn lighting(&self) -> LightingHub {
        self.lighting.clone()
    }

    pub fn detect_conflicts(&self) -> Vec<SoftwareConflict> {
        self.platform.conflicts()
    }

    // --- cooling ---------------------------------------------------------------------------

    /// Sets the preset to apply (the daemon passes the *effective* one, `auto` already resolved to
    /// `gaming` while a game runs).
    pub fn set_cooling_preset(&self, preset: CoolingPreset) {
        self.cooling.set_preset(preset);
    }

    pub fn set_gaming_mode(&self, gaming: bool) {
        self.cooling.set_gaming(gaming);
    }

    /// Runtime update of the watchdog thresholds (clamped; CPU critical 85-98 C).
    pub fn update_thermal_thresholds(&self, t: ThermalThresholds) {
        self.cooling.update_thresholds(t);
    }

    /// Curves, thresholds, effective state and reason: what the daemon's snapshot publishes.
    pub fn cooling_policy(&self) -> CoolingPolicy {
        self.cooling.policy()
    }

    pub fn is_thermal_failsafe_active(&self) -> bool {
        self.cooling.is_failsafe()
    }

    // --- OS updates ------------------------------------------------------------------------

    pub fn os_updates(&self) -> &OsUpdateState {
        &self.os_updates
    }

    pub fn set_os_updates(&mut self, state: OsUpdateState) {
        self.os_updates = state;
    }

    pub fn set_os_updates_in_progress(&mut self, in_progress: bool) {
        self.os_updates.in_progress = in_progress;
    }

    // --- manifest --------------------------------------------------------------------------

    fn current_caps(&mut self) -> (Caps, Option<Telemetry>) {
        let tel = self.cooling.telemetry();
        let fresh = tel.fresh(Instant::now()).cloned();
        let storage: Vec<(String, bool)> = self
            .storage
            .descriptors()
            .into_iter()
            .map(|d| (d.id, d.has_temperature))
            .collect();
        let kraken = self.kraken_info.as_ref().map(|k| k.1);
        let zones = self.lighting.zones();
        (
            caps_from(
                fresh.as_ref(),
                &zones,
                self.gpu_descriptors.len(),
                kraken,
                &storage,
            ),
            fresh,
        )
    }

    /// Builds the full Home Assistant device manifest (parent PC, child devices, entities).
    pub fn build_manifest(&mut self, scripts: &[AllowlistedScript]) -> DeviceManifest {
        self.system.refresh();
        let (now_caps, fresh) = self.current_caps();
        self.seen.absorb(&now_caps);
        let caps = self.seen.clone();
        let zones = self.lighting.zones();
        let host_info =
            self.system
                .host_info(&self.device_id, &self.sw_version, self.platform.mode());
        let mut child_devices = Vec::new();
        let mut entities = Vec::new();
        let sensor = sensor_desc;

        entities.extend(parent_system_entities(scripts));

        // --- CPU
        let cpu_name = self.system.cpu_brand();
        child_devices.push(ChildDeviceInfo {
            id: "cpu_0".into(),
            name: cpu_name.clone(),
            category: DeviceCategory::Cpu,
            manufacturer: system::infer_cpu_vendor(&cpu_name),
            model: Some(cpu_name),
            hw_version: None,
        });
        entities.push(sensor(
            "cpu_0_load".into(),
            "Load",
            Some("cpu_0".into()),
            None,
            Some("%"),
            Some("mdi:cpu-64-bit"),
            Some(1),
        ));
        entities.push(sensor(
            "cpu_0_frequency".into(),
            "Frequency",
            Some("cpu_0".into()),
            Some("frequency"),
            Some("MHz"),
            Some("mdi:sine-wave"),
            Some(0),
        ));
        if caps.cpu_temp {
            entities.push(sensor(
                "cpu_0_temperature".into(),
                "Temperature",
                Some("cpu_0".into()),
                Some("temperature"),
                Some("°C"),
                None,
                Some(1),
            ));
        }
        for (key, name) in &caps.ccds {
            entities.push(sensor(
                format!("cpu_0_{key}_temperature"),
                &format!("{name} Temperature"),
                Some("cpu_0".into()),
                Some("temperature"),
                Some("°C"),
                None,
                Some(1),
            ));
        }
        if caps.cpu_power {
            entities.push(sensor(
                "cpu_0_power".into(),
                "Power",
                Some("cpu_0".into()),
                Some("power"),
                Some("W"),
                None,
                Some(1),
            ));
        }

        // --- Memory
        let ram_total_gb = (self.system.total_memory_bytes() as f64 / 1_073_741_824.0).round();
        child_devices.push(ChildDeviceInfo {
            id: "memory_0".into(),
            name: format!("Memory ({ram_total_gb:.0} GB)"),
            category: DeviceCategory::Memory,
            manufacturer: None,
            model: Some(format!("{ram_total_gb:.0} GB RAM")),
            hw_version: None,
        });
        entities.push(sensor(
            "memory_0_usage".into(),
            "Usage",
            Some("memory_0".into()),
            None,
            Some("%"),
            Some("mdi:memory"),
            Some(1),
        ));
        entities.push(sensor(
            "memory_0_used".into(),
            "Used",
            Some("memory_0".into()),
            Some("data_size"),
            Some("GiB"),
            Some("mdi:memory"),
            Some(2),
        ));
        if caps.ram_temp {
            entities.push(sensor(
                "memory_0_temperature".into(),
                "Temperature",
                Some("memory_0".into()),
                Some("temperature"),
                Some("°C"),
                None,
                Some(1),
            ));
        }

        // --- GPUs
        let gpu_child_ids: Vec<String> = self
            .gpu_descriptors
            .iter()
            .map(|g| format!("gpu_{}", g.index))
            .collect();
        for g in &self.gpu_descriptors {
            let cid = format!("gpu_{}", g.index);
            child_devices.push(ChildDeviceInfo {
                id: cid.clone(),
                name: g.name.clone(),
                category: DeviceCategory::Gpu,
                manufacturer: Some(g.vendor.clone()),
                model: Some(g.name.clone()),
                hw_version: None,
            });
            for (suffix, name, class, unit, icon, prec) in [
                (
                    "temperature",
                    "Temperature",
                    Some("temperature"),
                    "°C",
                    None,
                    1u8,
                ),
                ("load", "Load", None, "%", Some("mdi:expansion-card"), 0),
                ("vram_usage", "VRAM Usage", None, "%", Some("mdi:memory"), 1),
                ("power", "Power", Some("power"), "W", None, 1),
                ("fan_speed", "Fan Speed", None, "%", Some("mdi:fan"), 0),
                (
                    "clock_graphics",
                    "Core Clock",
                    Some("frequency"),
                    "MHz",
                    Some("mdi:speedometer"),
                    0,
                ),
            ] {
                entities.push(sensor(
                    format!("{cid}_{suffix}"),
                    name,
                    Some(cid.clone()),
                    class,
                    Some(unit),
                    icon,
                    Some(prec),
                ));
            }
        }

        // --- Storage (one child per physical drive)
        for d in self.storage.descriptors() {
            child_devices.push(ChildDeviceInfo {
                id: d.id.clone(),
                name: d.name.clone(),
                category: DeviceCategory::Storage,
                manufacturer: d.manufacturer,
                model: Some(d.name),
                hw_version: None,
            });
            entities.push(sensor(
                format!("{}_usage", d.id),
                "Space Used",
                Some(d.id.clone()),
                None,
                Some("%"),
                Some("mdi:harddisk"),
                Some(1),
            ));
            if caps.storage_temp.contains(&d.id) {
                entities.push(sensor(
                    format!("{}_temperature", d.id),
                    "Temperature",
                    Some(d.id.clone()),
                    Some("temperature"),
                    Some("°C"),
                    None,
                    Some(0),
                ));
            }
        }

        // --- Motherboard
        let board_zone = zones
            .iter()
            .find(|z| z.id == ZONE_MOTHERBOARD && z.controllable);
        if board_zone.is_some()
            || caps.board_name.is_some()
            || caps.vrm
            || caps.chipset
            || !caps.fans.is_empty()
        {
            let board_name = caps
                .board_name
                .clone()
                .or_else(|| board_zone.map(|z| z.name.clone()))
                .unwrap_or_else(|| "Motherboard".into());
            child_devices.push(ChildDeviceInfo {
                id: "motherboard_0".into(),
                name: board_name.clone(),
                category: DeviceCategory::Motherboard,
                manufacturer: board_zone
                    .filter(|z| z.backend == "gigabyte_ite")
                    .map(|_| "Gigabyte".to_string()),
                model: Some(board_name),
                hw_version: None,
            });
            if caps.vrm {
                entities.push(sensor(
                    "motherboard_0_vrm_temperature".into(),
                    "VRM Temperature",
                    Some("motherboard_0".into()),
                    Some("temperature"),
                    Some("°C"),
                    None,
                    Some(1),
                ));
            }
            if caps.chipset {
                entities.push(sensor(
                    "motherboard_0_chipset_temperature".into(),
                    "Chipset Temperature",
                    Some("motherboard_0".into()),
                    Some("temperature"),
                    Some("°C"),
                    None,
                    Some(1),
                ));
            }
            for (suffix, name) in &caps.fans {
                entities.push(sensor(
                    format!("motherboard_0_fan_{suffix}"),
                    name,
                    Some("motherboard_0".into()),
                    None,
                    Some("RPM"),
                    Some("mdi:fan"),
                    Some(0),
                ));
            }
        }

        // --- Cooler (Kraken sensors, and/or an OpenRGB-driven cooler lighting zone)
        let cooler_zone = zones.iter().any(|z| z.id == ZONE_COOLER && z.controllable);
        if self.kraken_info.is_some() || cooler_zone {
            let model_name = self
                .kraken_info
                .as_ref()
                .map_or_else(|| "Watercooling AIO".to_string(), |k| k.0.clone());
            child_devices.push(ChildDeviceInfo {
                id: "cooler_0".into(),
                name: model_name.clone(),
                category: DeviceCategory::Cooler,
                manufacturer: self.kraken_info.as_ref().map(|_| "NZXT".to_string()),
                model: Some(model_name),
                hw_version: None,
            });
            if let Some((_, has_fan)) = &self.kraken_info {
                entities.push(sensor(
                    "cooler_0_liquid_temperature".into(),
                    "Liquid Temperature",
                    Some("cooler_0".into()),
                    Some("temperature"),
                    Some("°C"),
                    Some("mdi:coolant-temperature"),
                    Some(1),
                ));
                entities.push(sensor(
                    "cooler_0_pump_rpm".into(),
                    "Pump Speed",
                    Some("cooler_0".into()),
                    None,
                    Some("RPM"),
                    Some("mdi:pump"),
                    Some(0),
                ));
                entities.push(sensor(
                    "cooler_0_pump_duty".into(),
                    "Pump Duty",
                    Some("cooler_0".into()),
                    None,
                    Some("%"),
                    Some("mdi:pump"),
                    Some(0),
                ));
                if *has_fan {
                    entities.push(sensor(
                        "cooler_0_fan_rpm".into(),
                        "Radiator Fans",
                        Some("cooler_0".into()),
                        None,
                        Some("RPM"),
                        Some("mdi:fan"),
                        Some(0),
                    ));
                    entities.push(sensor(
                        "cooler_0_fan_duty".into(),
                        "Radiator Fan Duty",
                        Some("cooler_0".into()),
                        None,
                        Some("%"),
                        Some("mdi:fan"),
                        Some(0),
                    ));
                }
            }
        }

        // --- Lighting: exactly the zones a backend really drives
        for z in zones.iter().filter(|z| z.controllable) {
            let (child, name): (Option<String>, String) = match z.id.as_str() {
                ZONE_MOTHERBOARD => (Some("motherboard_0".into()), "Lighting".into()),
                ZONE_RAM => (Some("memory_0".into()), "Lighting".into()),
                ZONE_COOLER => (Some("cooler_0".into()), "Lighting".into()),
                ZONE_GPU => (gpu_child_ids.first().cloned(), "Lighting".into()),
                ZONE_FANS => (None, "Fan Lighting".into()),
                _ => (None, z.name.clone()),
            };
            entities.push(light_desc(&z.id, &name, child, false, z.led_count));
        }

        let _ = fresh;
        DeviceManifest {
            protocol_version: PROTOCOL_VERSION,
            host: host_info,
            child_devices,
            entities,
        }
    }

    // --- poll ------------------------------------------------------------------------------

    /// Samples everything and returns `(states, manifest_dirty)`. `manifest_dirty` is set when the
    /// hardware the manifest depends on changed (the bridge started delivering VRM / chipset / fan
    /// data, a drive appeared, ...): the caller rebuilds and republishes the manifest.
    ///
    /// Session-bound entities (active app, gaming mode, mic, webcam, volume, mute, output) are *not*
    /// produced here: they come from the agent.
    pub fn poll_states(&mut self) -> (Vec<EntityState>, bool) {
        let now = Utc::now();
        self.system.refresh();
        let mut states: Vec<EntityState> = Vec::new();
        macro_rules! push {
            ($id:expr, $value:expr $(,)?) => {
                states.push(EntityState {
                    id: $id,
                    value: $value,
                    attributes: BTreeMap::new(),
                    updated_at: now,
                })
            };
        }

        // CPU
        push!(
            "cpu_0_load".into(),
            EntityValue::Float(round1(f64::from(self.system.global_cpu_usage())))
        );
        if let Some(mhz) = self.system.cpu_frequency_mhz() {
            push!("cpu_0_frequency".into(), EntityValue::Int(mhz as i64));
        }

        let tel_all = self.cooling.telemetry();
        let tel = tel_all.fresh(Instant::now());
        let empty_deep = DeepSensorSnapshot::default();
        if let Some(t) = tel {
            let deep = t.deep.as_ref().unwrap_or(&empty_deep);
            if let Some(c) = t.cpu.package_c.or(deep.cpu_temp_c) {
                push!("cpu_0_temperature".into(), EntityValue::Float(round1(c)));
            }
            for (name, c) in &t.cpu.ccd_c {
                push!(
                    format!("cpu_0_{}_temperature", ccd_key(name)),
                    EntityValue::Float(round1(*c))
                );
            }
            for ccd in &deep.cpu_ccd_temps_c {
                if let Some(v) = ccd.value {
                    push!(
                        format!("cpu_0_{}_temperature", ccd_key(&ccd.name)),
                        EntityValue::Float(round1(v))
                    );
                }
            }
            if let Some(w) = deep.cpu_power_w {
                push!("cpu_0_power".into(), EntityValue::Float(round1(w)));
            }
            if let Some(v) = deep.ram_temp_c {
                push!("memory_0_temperature".into(), EntityValue::Float(round1(v)));
            }
            if let Some(v) = deep.motherboard_vrm_temp_c {
                push!(
                    "motherboard_0_vrm_temperature".into(),
                    EntityValue::Float(round1(v))
                );
            }
            if let Some(v) = deep.motherboard_chipset_temp_c {
                push!(
                    "motherboard_0_chipset_temperature".into(),
                    EntityValue::Float(round1(v))
                );
            }
            for f in &deep.case_fans {
                if let Some(rpm) = f.rpm {
                    push!(
                        format!("motherboard_0_fan_{}", fan_entity_suffix(&f.id)),
                        EntityValue::Int(rpm.round() as i64)
                    );
                }
            }
        }

        // Memory
        push!(
            "memory_0_usage".into(),
            EntityValue::Float(round1(self.system.memory_usage_percent()))
        );
        push!(
            "memory_0_used".into(),
            EntityValue::Float(
                (self.system.used_memory_bytes() as f64 / 1_073_741_824.0 * 100.0).round() / 100.0
            ),
        );

        // GPUs
        if let Some(t) = tel {
            for s in &t.gpus {
                let cid = format!("gpu_{}", s.index);
                if let Some(v) = s.temperature_c {
                    push!(format!("{cid}_temperature"), EntityValue::Float(round1(v)));
                }
                if let Some(v) = s.utilization_pct {
                    push!(format!("{cid}_load"), EntityValue::Float(f64::from(v)));
                }
                if let Some(v) = s.vram_usage_pct {
                    let mut attrs = BTreeMap::new();
                    if let (Some(u), Some(t)) = (s.vram_used_mib, s.vram_total_mib) {
                        attrs.insert("vram_used_mib".to_string(), serde_json::json!(u));
                        attrs.insert("vram_total_mib".to_string(), serde_json::json!(t));
                    }
                    states.push(EntityState {
                        id: format!("{cid}_vram_usage"),
                        value: EntityValue::Float(round1(v)),
                        attributes: attrs,
                        updated_at: now,
                    });
                }
                if let Some(v) = s.power_watts {
                    push!(format!("{cid}_power"), EntityValue::Float(round1(v)));
                }
                if let Some(v) = s.fan_speed_pct {
                    push!(format!("{cid}_fan_speed"), EntityValue::Int(i64::from(v)));
                }
                if let Some(v) = s.graphics_clock_mhz {
                    push!(
                        format!("{cid}_clock_graphics"),
                        EntityValue::Int(i64::from(v))
                    );
                }
            }
            // Kraken
            if let KrakenReading::Ok(k) = &t.kraken {
                if let Some(v) = k.liquid_temp_c {
                    push!(
                        "cooler_0_liquid_temperature".into(),
                        EntityValue::Float(round1(v))
                    );
                }
                if let Some(v) = k.pump_rpm {
                    push!("cooler_0_pump_rpm".into(), EntityValue::Int(i64::from(v)));
                }
                if let Some(v) = k.pump_duty_pct {
                    push!("cooler_0_pump_duty".into(), EntityValue::Int(i64::from(v)));
                }
                if let Some(v) = k.fan_rpm {
                    push!("cooler_0_fan_rpm".into(), EntityValue::Int(i64::from(v)));
                }
                if let Some(v) = k.fan_duty_pct {
                    push!("cooler_0_fan_duty".into(), EntityValue::Int(i64::from(v)));
                }
            }
        }

        // Storage
        let drives = self.storage.poll();
        let storage_caps: Vec<(String, bool)> = drives
            .iter()
            .map(|d| (d.id.clone(), d.temperature_c.is_some()))
            .collect();
        for d in &drives {
            push!(
                format!("{}_usage", d.id),
                EntityValue::Float(round1(d.used_percent))
            );
            if let Some(t) = d.temperature_c {
                push!(
                    format!("{}_temperature", d.id),
                    EntityValue::Float(round1(t))
                );
            }
        }

        // Cooling
        let policy = self.cooling.policy();
        push!(
            "cooling_preset".into(),
            EntityValue::Text(policy.effective_preset.as_ha_option().into())
        );
        push!(
            "thermal_failsafe".into(),
            EntityValue::Bool(policy.state == ctrlmate_protocol::CoolingState::Failsafe),
        );

        // Lighting
        for (id, st) in self.lighting.states() {
            push!(id, EntityValue::Light(st));
        }

        // OS updates: the reboot flag is live, never sticky.
        self.os_updates.reboot_required = updates::is_reboot_required();
        push!(
            "os_updates".into(),
            EntityValue::Update(UpdatesInfo::from(&self.os_updates))
        );
        push!(
            "reboot_required".into(),
            EntityValue::Bool(self.os_updates.reboot_required)
        );

        // Did the hardware the manifest depends on change?
        let zones = self.lighting.zones();
        let kraken = self.kraken_info.as_ref().map(|k| k.1);
        let caps = caps_from(
            tel,
            &zones,
            self.gpu_descriptors.len(),
            kraken,
            &storage_caps,
        );
        let dirty = self.seen.absorb(&caps) && self.seen != Caps::default();
        (states, dirty)
    }
}

impl Drop for HardwareCollector {
    fn drop(&mut self) {
        // Order matters: stop the supervisor first (it could restart the bridge), then stop the
        // watchdog and hand Kraken, GPU fans and motherboard PWM back to firmware control.
        self.supervisor.stop();
        self.cooling.shutdown();
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn sensor_desc(
    id: String,
    name: &str,
    child_device_id: Option<String>,
    device_class: Option<&str>,
    unit: Option<&str>,
    icon: Option<&str>,
    precision: Option<u8>,
) -> EntityDescriptor {
    EntityDescriptor {
        id,
        name: name.into(),
        platform: EntityPlatform::Sensor,
        child_device_id,
        device_class: device_class.map(str::to_string),
        state_class: Some(StateClass::Measurement),
        unit_of_measurement: unit.map(str::to_string),
        icon: icon.map(str::to_string),
        entity_category: None,
        suggested_display_precision: precision,
        options: vec![],
        number_range: None,
        light_caps: None,
        exposed: true,
    }
}

fn light_desc(
    id: &str,
    name: &str,
    child_device_id: Option<String>,
    is_master_zone: bool,
    led_count: u32,
) -> EntityDescriptor {
    EntityDescriptor {
        id: id.into(),
        name: name.into(),
        platform: EntityPlatform::Light,
        child_device_id,
        device_class: None,
        state_class: None,
        unit_of_measurement: None,
        icon: Some("mdi:led-strip-variant".into()),
        entity_category: None,
        suggested_display_precision: None,
        options: vec![],
        number_range: None,
        light_caps: Some(LightCapabilities {
            is_master_zone,
            led_count: led_count.min(u32::from(u16::MAX)) as u16,
            ..LightCapabilities::default()
        }),
        exposed: true,
    }
}

fn plain(id: &str, name: &str, platform: EntityPlatform, icon: &str) -> EntityDescriptor {
    EntityDescriptor {
        id: id.into(),
        name: name.into(),
        platform,
        child_device_id: None,
        device_class: None,
        state_class: None,
        unit_of_measurement: None,
        icon: Some(icon.into()),
        entity_category: None,
        suggested_display_precision: None,
        options: vec![],
        number_range: None,
        light_caps: None,
        exposed: true,
    }
}

fn button_desc(id: &str, name: &str, icon: &str) -> EntityDescriptor {
    plain(id, name, EntityPlatform::Button, icon)
}

/// Parent PC entities. The session-bound ones (`active_application`, `gaming_mode`, `microphone_in_use`,
/// `webcam_in_use`, `master_volume`, `audio_mute`, `audio_output`) are declared here but their state and
/// the `audio_output` options come from the agent through the daemon.
fn parent_system_entities(scripts: &[AllowlistedScript]) -> Vec<EntityDescriptor> {
    let mut ents = vec![
        light_desc(ZONE_GLOBAL, "PC Lighting", None, true, 0),
        EntityDescriptor {
            options: CoolingPreset::all_ha_options(),
            ..plain(
                "cooling_preset",
                "Cooling Mode",
                EntityPlatform::Select,
                "mdi:fan",
            )
        },
        EntityDescriptor {
            device_class: Some("safety".into()),
            entity_category: Some(EntityCategory::Diagnostic),
            ..plain(
                "thermal_failsafe",
                "Thermal Failsafe",
                EntityPlatform::BinarySensor,
                "mdi:thermometer-alert",
            )
        },
        plain(
            "active_application",
            "Active Application",
            EntityPlatform::Sensor,
            "mdi:application",
        ),
        EntityDescriptor {
            device_class: Some("running".into()),
            ..plain(
                "gaming_mode",
                "Gaming Mode",
                EntityPlatform::BinarySensor,
                "mdi:controller",
            )
        },
        EntityDescriptor {
            entity_category: Some(EntityCategory::Diagnostic),
            ..plain(
                "os_updates",
                "OS Updates",
                EntityPlatform::Update,
                "mdi:update",
            )
        },
        EntityDescriptor {
            device_class: Some("problem".into()),
            entity_category: Some(EntityCategory::Diagnostic),
            ..plain(
                "reboot_required",
                "Reboot Required",
                EntityPlatform::BinarySensor,
                "mdi:restart-alert",
            )
        },
        EntityDescriptor {
            device_class: Some("running".into()),
            ..plain(
                "microphone_in_use",
                "Microphone In Use",
                EntityPlatform::BinarySensor,
                "mdi:microphone",
            )
        },
        EntityDescriptor {
            device_class: Some("running".into()),
            ..plain(
                "webcam_in_use",
                "Webcam In Use",
                EntityPlatform::BinarySensor,
                "mdi:webcam",
            )
        },
        EntityDescriptor {
            unit_of_measurement: Some("%".into()),
            suggested_display_precision: Some(0),
            number_range: Some(NumberRange {
                min: 0.0,
                max: 100.0,
                step: 1.0,
            }),
            ..plain(
                "master_volume",
                "Master Volume",
                EntityPlatform::Number,
                "mdi:volume-high",
            )
        },
        plain(
            "audio_mute",
            "Audio Mute",
            EntityPlatform::Switch,
            "mdi:volume-mute",
        ),
        // Options are filled by the daemon from the agent's list of outputs.
        plain(
            "audio_output",
            "Audio Output",
            EntityPlatform::Select,
            "mdi:speaker",
        ),
        button_desc("check_os_updates", "Check OS Updates", "mdi:update"),
        button_desc("power_shutdown", "Shutdown", "mdi:power"),
        button_desc("power_restart", "Restart", "mdi:restart"),
        button_desc("power_sleep", "Sleep", "mdi:power-sleep"),
        button_desc("power_hibernate", "Hibernate", "mdi:snowflake"),
        button_desc("power_lock", "Lock Session", "mdi:lock"),
        button_desc("display_off", "Turn Off Displays", "mdi:monitor-off"),
    ];
    for script in scripts.iter().filter(|s| s.enabled) {
        ents.push(button_desc(
            &format!("script_{}", script.id),
            &script.name,
            "mdi:script-text-play-outline",
        ));
    }
    ents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_ids_are_stable_and_independent_of_rpm_or_order() {
        assert_eq!(fan_entity_suffix("/lpc/it8688e/0/fan/1"), "it8688e_1");
        assert_eq!(fan_entity_suffix("/lpc/nct6798d/0/fan/3"), "nct6798d_3");
        assert_eq!(fan_entity_suffix("weird id!"), "weird_id");
    }

    #[test]
    fn ccd_keys() {
        assert_eq!(ccd_key("CCD1"), "ccd1");
        assert_eq!(ccd_key("Tccd2"), "ccd2");
        assert_eq!(ccd_key("Core Max"), "core_max");
    }

    #[test]
    fn caps_only_grow_for_sensors_and_dirty_means_a_real_change() {
        let mut seen = Caps::default();
        let mut now = Caps {
            cpu_temp: true,
            ..Caps::default()
        };
        assert!(seen.absorb(&now));
        assert!(!seen.absorb(&now), "no change, no churn");
        // The bridge starts delivering VRM + a fan: manifest must be rebuilt.
        now.vrm = true;
        now.fans.insert("it8688e_1".into(), "Fan #1".into());
        assert!(seen.absorb(&now));
        // The bridge pauses (anti-cheat): the entities stay registered, no manifest churn.
        let paused = Caps::default();
        let changed = seen.absorb(&paused);
        assert!(!changed || seen.vrm, "{seen:?}");
        assert!(seen.vrm && seen.cpu_temp && !seen.fans.is_empty());
    }

    #[test]
    fn parent_entities_declare_session_entities_and_no_phantom_lights() {
        let ents = parent_system_entities(&[]);
        for id in [
            "gaming_mode",
            "microphone_in_use",
            "master_volume",
            "audio_output",
            "cooling_preset",
        ] {
            assert!(ents.iter().any(|e| e.id == id), "{id}");
        }
        let lights: Vec<_> = ents
            .iter()
            .filter(|e| e.platform == EntityPlatform::Light)
            .collect();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].id, "light_global");
        let preset = ents.iter().find(|e| e.id == "cooling_preset").unwrap();
        assert_eq!(preset.options.len(), 5);
        assert!(!preset.options.contains(&"full_speed".to_string()));
    }

    #[test]
    fn disabled_scripts_get_no_button() {
        let s = |id: &str, enabled| AllowlistedScript {
            id: id.into(),
            name: id.into(),
            executable: "/x".into(),
            args: vec![],
            working_dir: None,
            enabled,
        };
        let ents = parent_system_entities(&[s("a", true), s("b", false)]);
        assert!(ents.iter().any(|e| e.id == "script_a"));
        assert!(!ents.iter().any(|e| e.id == "script_b"));
    }
}
