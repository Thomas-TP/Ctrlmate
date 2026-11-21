//! Cross-platform hardware & OS telemetry collector (Zero-Kernel / Anti-Cheat Safe by default).

pub mod conflicts;
pub mod gpu;
pub mod hid;
pub mod os_control;
pub mod smart_poll;
pub mod storage;
pub mod system;

use chrono::Utc;
use ctrlmate_cooling::{CoolingWatchdog, ThermalReadings, ThermalSafetyConfig};
use ctrlmate_protocol::{
    ChildDeviceInfo, CoolingPreset, DetectedHidDevice, DeviceCategory, DeviceManifest,
    EntityCategory, EntityDescriptor, EntityPlatform, EntityState, EntityValue,
    LightCapabilities, LightState, NumberRange, OsUpdateState, SecurityMode, SoftwareConflict,
    StateClass, PROTOCOL_VERSION,
};
use ctrlmate_rgb::LightingEngine;
use std::collections::BTreeMap;
use tracing::{debug, info};

/// Central hardware & telemetry state collector running inside `ctrlmated`.
pub struct HardwareCollector {
    device_id: String,
    sw_version: String,
    security_mode: SecurityMode,
    system: system::SystemProvider,
    gpu: gpu::GpuProvider,
    hid_devices: Vec<DetectedHidDevice>,
    lighting: LightingEngine,
    cooling: CoolingWatchdog,
    os_updates: OsUpdateState,
    master_volume: f64,
    audio_muted: bool,
    audio_output: String,
}

impl HardwareCollector {
    pub fn new(device_id: String, sw_version: String, security_mode: SecurityMode) -> Self {
        let system = system::SystemProvider::new();
        let gpu = gpu::GpuProvider::new();
        let hid_devices = hid::scan_known_hid_devices();
        let mut lighting = LightingEngine::new();

        // Register logical lighting zones discovered on this system.
        lighting.register_zone("light_global", LightState::default());
        for dev in &hid_devices {
            match dev.vendor_id {
                0x048D => lighting.register_zone("light_motherboard", LightState::default()),
                0x1E71 => lighting.register_zone("light_cooler", LightState::default()),
                0x0B05 => lighting.register_zone("light_gpu", LightState::default()),
                0x1B1C | 0x0CF2 => lighting.register_zone("light_fans", LightState::default()),
                _ => {}
            }
        }

        info!(
            "HardwareCollector initialized (GPU count: {}, known USB HID controllers: {})",
            gpu.device_count(),
            hid_devices.len()
        );

        Self {
            device_id,
            sw_version,
            security_mode,
            system,
            gpu,
            hid_devices,
            lighting,
            cooling: CoolingWatchdog::new(ThermalSafetyConfig::default()),
            os_updates: OsUpdateState {
                installed_version: system::os_version_string(),
                latest_version: system::os_version_string(),
                pending_count: 0,
                pending_titles: Vec::new(),
                in_progress: false,
                reboot_required: false,
                last_checked: None,
            },
            master_volume: 50.0,
            audio_muted: false,
            audio_output: "Default Output".into(),
        }
    }

    pub fn security_mode(&self) -> SecurityMode {
        self.security_mode
    }

    pub fn set_security_mode(&mut self, mode: SecurityMode) {
        self.security_mode = mode;
    }

    pub fn hid_devices(&self) -> &[DetectedHidDevice] {
        &self.hid_devices
    }

    pub fn rescan_hid(&mut self) {
        self.hid_devices = hid::scan_known_hid_devices();
    }

    pub fn lighting_mut(&mut self) -> &mut LightingEngine {
        &mut self.lighting
    }

    pub fn cooling_mut(&mut self) -> &mut CoolingWatchdog {
        &mut self.cooling
    }

    pub fn is_thermal_failsafe_active(&self) -> bool {
        self.cooling.is_failsafe_active()
    }

    pub fn set_audio_state(&mut self, volume: Option<f64>, muted: Option<bool>, output: Option<String>) {
        if let Some(v) = volume {
            self.master_volume = v.clamp(0.0, 100.0);
        }
        if let Some(m) = muted {
            self.audio_muted = m;
        }
        if let Some(o) = output {
            self.audio_output = o;
        }
    }

    pub fn detect_conflicts(&self) -> Vec<SoftwareConflict> {
        conflicts::detect_running_conflicts(&self.system.sys())
    }

    /// Builds the full Home Assistant 2026.11 Device Manifest (Parent PC + Child Devices + Entities).
    pub fn build_manifest(&mut self) -> DeviceManifest {
        self.system.refresh();
        let host_info = self.system.host_info(&self.device_id, &self.sw_version, self.security_mode);

        let mut child_devices = Vec::new();
        let mut entities = Vec::new();

        // --- Parent PC Entities (Power, OS Updates, Session, Privacy, Global Lighting & Cooling) ---
        entities.extend(self.parent_system_entities());

        // --- Child Device: CPU ---
        let cpu_name = self.system.cpu_brand();
        child_devices.push(ChildDeviceInfo {
            id: "cpu_0".into(),
            name: cpu_name.clone(),
            category: DeviceCategory::Cpu,
            manufacturer: system::infer_cpu_vendor(&cpu_name),
            model: Some(cpu_name),
            hw_version: None,
        });
        entities.push(EntityDescriptor {
            id: "cpu_0_load".into(),
            name: "Load".into(),
            platform: EntityPlatform::Sensor,
            child_device_id: Some("cpu_0".into()),
            device_class: None,
            state_class: Some(StateClass::Measurement),
            unit_of_measurement: Some("%".into()),
            icon: Some("mdi:cpu-64-bit".into()),
            entity_category: None,
            suggested_display_precision: Some(1),
            options: vec![],
            number_range: None,
            light_caps: None,
            exposed: true,
        });
        entities.push(EntityDescriptor {
            id: "cpu_0_frequency".into(),
            name: "Frequency".into(),
            platform: EntityPlatform::Sensor,
            child_device_id: Some("cpu_0".into()),
            device_class: Some("frequency".into()),
            state_class: Some(StateClass::Measurement),
            unit_of_measurement: Some("MHz".into()),
            icon: Some("mdi:sine-wave".into()),
            entity_category: None,
            suggested_display_precision: Some(0),
            options: vec![],
            number_range: None,
            light_caps: None,
            exposed: true,
        });
        #[cfg(target_os = "linux")]
        entities.push(EntityDescriptor {
            id: "cpu_0_temperature".into(),
            name: "Temperature".into(),
            platform: EntityPlatform::Sensor,
            child_device_id: Some("cpu_0".into()),
            device_class: Some("temperature".into()),
            state_class: Some(StateClass::Measurement),
            unit_of_measurement: Some("°C".into()),
            icon: None,
            entity_category: None,
            suggested_display_precision: Some(1),
            options: vec![],
            number_range: None,
            light_caps: None,
            exposed: true,
        });

        // --- Child Device: Memory (RAM) ---
        let ram_total_gb = (self.system.total_memory_bytes() as f64 / 1_073_741_824.0).round();
        child_devices.push(ChildDeviceInfo {
            id: "memory_0".into(),
            name: format!("Memory ({ram_total_gb:.0} GB)"),
            category: DeviceCategory::Memory,
            manufacturer: None,
            model: Some(format!("{ram_total_gb:.0} GB RAM")),
            hw_version: None,
        });
        entities.push(EntityDescriptor {
            id: "memory_0_usage".into(),
            name: "Usage".into(),
            platform: EntityPlatform::Sensor,
            child_device_id: Some("memory_0".into()),
            device_class: None,
            state_class: Some(StateClass::Measurement),
            unit_of_measurement: Some("%".into()),
            icon: Some("mdi:memory".into()),
            entity_category: None,
            suggested_display_precision: Some(1),
            options: vec![],
            number_range: None,
            light_caps: None,
            exposed: true,
        });
        entities.push(EntityDescriptor {
            id: "memory_0_used".into(),
            name: "Used".into(),
            platform: EntityPlatform::Sensor,
            child_device_id: Some("memory_0".into()),
            device_class: Some("data_size".into()),
            state_class: Some(StateClass::Measurement),
            unit_of_measurement: Some("GiB".into()),
            icon: Some("mdi:memory".into()),
            entity_category: None,
            suggested_display_precision: Some(2),
            options: vec![],
            number_range: None,
            light_caps: None,
            exposed: true,
        });

        // --- Child Devices: GPUs (NVIDIA NVML / Linux DRM) ---
        for gpu_info in self.gpu.descriptors() {
            let cid = format!("gpu_{}", gpu_info.index);
            child_devices.push(ChildDeviceInfo {
                id: cid.clone(),
                name: gpu_info.name.clone(),
                category: DeviceCategory::Gpu,
                manufacturer: Some(gpu_info.vendor.clone()),
                model: Some(gpu_info.name.clone()),
                hw_version: None,
            });

            entities.push(sensor_desc(
                format!("{cid}_temperature"),
                "Temperature",
                Some(cid.clone()),
                Some("temperature"),
                Some("°C"),
                None,
                Some(1),
            ));
            entities.push(sensor_desc(
                format!("{cid}_load"),
                "Load",
                Some(cid.clone()),
                None,
                Some("%"),
                Some("mdi:expansion-card"),
                Some(0),
            ));
            entities.push(sensor_desc(
                format!("{cid}_vram_usage"),
                "VRAM Usage",
                Some(cid.clone()),
                None,
                Some("%"),
                Some("mdi:memory"),
                Some(1),
            ));
            entities.push(sensor_desc(
                format!("{cid}_power"),
                "Power",
                Some(cid.clone()),
                Some("power"),
                Some("W"),
                None,
                Some(1),
            ));
            entities.push(sensor_desc(
                format!("{cid}_fan_speed"),
                "Fan Speed",
                Some(cid.clone()),
                None,
                Some("%"),
                Some("mdi:fan"),
                Some(0),
            ));
            entities.push(sensor_desc(
                format!("{cid}_clock_graphics"),
                "Core Clock",
                Some(cid.clone()),
                Some("frequency"),
                Some("MHz"),
                Some("mdi:speedometer"),
                Some(0),
            ));
        }

        // --- Child Devices: Storage / NVMe Drives ---
        let drives = storage::enumerate_drives();
        for drv in drives {
            child_devices.push(ChildDeviceInfo {
                id: drv.id.clone(),
                name: drv.name.clone(),
                category: DeviceCategory::Storage,
                manufacturer: drv.manufacturer,
                model: Some(drv.name),
                hw_version: None,
            });
            entities.push(sensor_desc(
                format!("{}_usage", drv.id),
                "Space Used",
                Some(drv.id.clone()),
                None,
                Some("%"),
                Some("mdi:harddisk"),
                Some(1),
            ));
            if drv.has_temperature {
                entities.push(sensor_desc(
                    format!("{}_temperature", drv.id),
                    "Temperature",
                    Some(drv.id.clone()),
                    Some("temperature"),
                    Some("°C"),
                    None,
                    Some(0),
                ));
            }
        }

        // --- Child Devices: Lighting & Cooler Zones detected via USB HID ---
        if self.lighting.zone_state("light_motherboard").is_some() {
            child_devices.push(ChildDeviceInfo {
                id: "motherboard_0".into(),
                name: "Motherboard".into(),
                category: DeviceCategory::Motherboard,
                manufacturer: Some("Gigabyte / ITE".into()),
                model: Some("Motherboard RGB Controller".into()),
                hw_version: None,
            });
            entities.push(light_desc("light_motherboard", "Lighting", Some("motherboard_0".into()), false));
        }
        if self.lighting.zone_state("light_cooler").is_some() {
            child_devices.push(ChildDeviceInfo {
                id: "cooler_0".into(),
                name: "Watercooling AIO".into(),
                category: DeviceCategory::Cooler,
                manufacturer: Some("NZXT".into()),
                model: Some("Kraken AIO".into()),
                hw_version: None,
            });
            entities.push(light_desc("light_cooler", "Lighting", Some("cooler_0".into()), false));
        }
        if self.lighting.zone_state("light_gpu").is_some() {
            let gpu_child = child_devices
                .iter()
                .find(|d| d.category == DeviceCategory::Gpu)
                .map(|d| d.id.clone());
            entities.push(light_desc("light_gpu", "Lighting", gpu_child, false));
        }

        DeviceManifest {
            protocol_version: PROTOCOL_VERSION,
            host: host_info,
            child_devices,
            entities,
        }
    }

    fn parent_system_entities(&self) -> Vec<EntityDescriptor> {
        vec![
            light_desc("light_global", "PC Lighting", None, true),
            EntityDescriptor {
                id: "cooling_preset".into(),
                name: "Cooling Mode".into(),
                platform: EntityPlatform::Select,
                child_device_id: None,
                device_class: None,
                state_class: None,
                unit_of_measurement: None,
                icon: Some("mdi:fan".into()),
                entity_category: None,
                suggested_display_precision: None,
                options: CoolingPreset::all_ha_options(),
                number_range: None,
                light_caps: None,
                exposed: true,
            },
            EntityDescriptor {
                id: "thermal_failsafe".into(),
                name: "Thermal Failsafe".into(),
                platform: EntityPlatform::BinarySensor,
                child_device_id: None,
                device_class: Some("safety".into()),
                state_class: None,
                unit_of_measurement: None,
                icon: Some("mdi:thermometer-alert".into()),
                entity_category: Some(EntityCategory::Diagnostic),
                suggested_display_precision: None,
                options: vec![],
                number_range: None,
                light_caps: None,
                exposed: true,
            },
            EntityDescriptor {
                id: "os_updates".into(),
                name: "OS Updates".into(),
                platform: EntityPlatform::Update,
                child_device_id: None,
                device_class: None,
                state_class: None,
                unit_of_measurement: None,
                icon: Some("mdi:update".into()),
                entity_category: Some(EntityCategory::Diagnostic),
                suggested_display_precision: None,
                options: vec![],
                number_range: None,
                light_caps: None,
                exposed: true,
            },
            EntityDescriptor {
                id: "microphone_in_use".into(),
                name: "Microphone In Use".into(),
                platform: EntityPlatform::BinarySensor,
                child_device_id: None,
                device_class: Some("running".into()),
                state_class: None,
                unit_of_measurement: None,
                icon: Some("mdi:microphone".into()),
                entity_category: None,
                suggested_display_precision: None,
                options: vec![],
                number_range: None,
                light_caps: None,
                exposed: true,
            },
            EntityDescriptor {
                id: "webcam_in_use".into(),
                name: "Webcam In Use".into(),
                platform: EntityPlatform::BinarySensor,
                child_device_id: None,
                device_class: Some("running".into()),
                state_class: None,
                unit_of_measurement: None,
                icon: Some("mdi:webcam".into()),
                entity_category: None,
                suggested_display_precision: None,
                options: vec![],
                number_range: None,
                light_caps: None,
                exposed: true,
            },
            EntityDescriptor {
                id: "master_volume".into(),
                name: "Master Volume".into(),
                platform: EntityPlatform::Number,
                child_device_id: None,
                device_class: None,
                state_class: None,
                unit_of_measurement: Some("%".into()),
                icon: Some("mdi:volume-high".into()),
                entity_category: None,
                suggested_display_precision: Some(0),
                options: vec![],
                number_range: Some(NumberRange {
                    min: 0.0,
                    max: 100.0,
                    step: 1.0,
                }),
                light_caps: None,
                exposed: true,
            },
            EntityDescriptor {
                id: "audio_mute".into(),
                name: "Audio Mute".into(),
                platform: EntityPlatform::Switch,
                child_device_id: None,
                device_class: None,
                state_class: None,
                unit_of_measurement: None,
                icon: Some("mdi:volume-mute".into()),
                entity_category: None,
                suggested_display_precision: None,
                options: vec![],
                number_range: None,
                light_caps: None,
                exposed: true,
            },
            EntityDescriptor {
                id: "audio_output".into(),
                name: "Audio Output".into(),
                platform: EntityPlatform::Select,
                child_device_id: None,
                device_class: None,
                state_class: None,
                unit_of_measurement: None,
                icon: Some("mdi:speaker".into()),
                entity_category: None,
                suggested_display_precision: None,
                options: vec!["Default Output".into()],
                number_range: None,
                light_caps: None,
                exposed: true,
            },
            button_desc("power_shutdown", "Shutdown", "mdi:power"),
            button_desc("power_restart", "Restart", "mdi:restart"),
            button_desc("power_sleep", "Sleep", "mdi:power-sleep"),
            button_desc("power_hibernate", "Hibernate", "mdi:snowflake"),
            button_desc("power_lock", "Lock Session", "mdi:lock"),
            button_desc("display_off", "Turn Off Displays", "mdi:monitor-off"),
        ]
    }

    /// Samples all hardware providers and produces the current entity state list.
    pub fn poll_states(&mut self) -> Vec<EntityState> {
        let now = Utc::now();
        self.system.refresh();
        let mut states = Vec::new();

        let mut thermal = ThermalReadings::default();

        // CPU
        let cpu_load = f64::from(self.system.global_cpu_usage());
        states.push(EntityState {
            id: "cpu_0_load".into(),
            value: EntityValue::Float(round1(cpu_load)),
            attributes: BTreeMap::new(),
            updated_at: now,
        });
        states.push(EntityState {
            id: "cpu_0_frequency".into(),
            value: EntityValue::Int(self.system.cpu_frequency_mhz() as i64),
            attributes: BTreeMap::new(),
            updated_at: now,
        });

        #[cfg(target_os = "linux")]
        if let Some(cpu_temp) = system::read_linux_cpu_temp_c() {
            thermal.cpu_c = Some(cpu_temp);
            states.push(EntityState {
                id: "cpu_0_temperature".into(),
                value: EntityValue::Float(round1(cpu_temp)),
                attributes: BTreeMap::new(),
                updated_at: now,
            });
        }

        // Memory
        let mem_used_gib = self.system.used_memory_bytes() as f64 / 1_073_741_824.0;
        let mem_pct = self.system.memory_usage_percent();
        states.push(EntityState {
            id: "memory_0_usage".into(),
            value: EntityValue::Float(round1(mem_pct)),
            attributes: BTreeMap::new(),
            updated_at: now,
        });
        states.push(EntityState {
            id: "memory_0_used".into(),
            value: EntityValue::Float((mem_used_gib * 100.0).round() / 100.0),
            attributes: BTreeMap::new(),
            updated_at: now,
        });

        // GPUs
        for sample in self.gpu.poll() {
            let cid = format!("gpu_{}", sample.index);
            if let Some(temp) = sample.temperature_c {
                thermal.gpu_c = Some(thermal.gpu_c.map_or(temp, |prev| prev.max(temp)));
                states.push(EntityState {
                    id: format!("{cid}_temperature"),
                    value: EntityValue::Float(round1(temp)),
                    attributes: BTreeMap::new(),
                    updated_at: now,
                });
            }
            if let Some(load) = sample.utilization_pct {
                states.push(EntityState {
                    id: format!("{cid}_load"),
                    value: EntityValue::Float(f64::from(load)),
                    attributes: BTreeMap::new(),
                    updated_at: now,
                });
            }
            if let Some(vram_pct) = sample.vram_usage_pct {
                let mut attrs = BTreeMap::new();
                if let (Some(used), Some(total)) = (sample.vram_used_mib, sample.vram_total_mib) {
                    attrs.insert("vram_used_mib".into(), serde_json::json!(used));
                    attrs.insert("vram_total_mib".into(), serde_json::json!(total));
                }
                states.push(EntityState {
                    id: format!("{cid}_vram_usage"),
                    value: EntityValue::Float(round1(vram_pct)),
                    attributes: attrs,
                    updated_at: now,
                });
            }
            if let Some(watts) = sample.power_watts {
                states.push(EntityState {
                    id: format!("{cid}_power"),
                    value: EntityValue::Float(round1(watts)),
                    attributes: BTreeMap::new(),
                    updated_at: now,
                });
            }
            if let Some(fan) = sample.fan_speed_pct {
                states.push(EntityState {
                    id: format!("{cid}_fan_speed"),
                    value: EntityValue::Int(i64::from(fan)),
                    attributes: BTreeMap::new(),
                    updated_at: now,
                });
            }
            if let Some(clk) = sample.graphics_clock_mhz {
                states.push(EntityState {
                    id: format!("{cid}_clock_graphics"),
                    value: EntityValue::Int(i64::from(clk)),
                    attributes: BTreeMap::new(),
                    updated_at: now,
                });
            }
        }

        // Storage drives
        for drv in storage::poll_drives() {
            states.push(EntityState {
                id: format!("{}_usage", drv.id),
                value: EntityValue::Float(round1(drv.used_percent)),
                attributes: BTreeMap::new(),
                updated_at: now,
            });
            if let Some(temp) = drv.temperature_c {
                states.push(EntityState {
                    id: format!("{}_temperature", drv.id),
                    value: EntityValue::Float(round1(temp)),
                    attributes: BTreeMap::new(),
                    updated_at: now,
                });
            }
        }

        // Thermal watchdog evaluation
        let cooling_targets = self.cooling.evaluate(thermal);
        if cooling_targets.failsafe_triggered {
            debug!("Thermal failsafe active! Effective preset: {:?}", cooling_targets.effective_preset);
        }
        states.push(EntityState {
            id: "cooling_preset".into(),
            value: EntityValue::Text(cooling_targets.effective_preset.as_ha_option().into()),
            attributes: BTreeMap::new(),
            updated_at: now,
        });
        states.push(EntityState {
            id: "thermal_failsafe".into(),
            value: EntityValue::Bool(cooling_targets.failsafe_triggered),
            attributes: BTreeMap::new(),
            updated_at: now,
        });

        // Lighting zones
        for (id, st) in self.lighting.all_zones() {
            states.push(EntityState {
                id: id.clone(),
                value: EntityValue::Light(st.clone()),
                attributes: BTreeMap::new(),
                updated_at: now,
            });
        }

        // Privacy indicators (Microphone & Webcam)
        let privacy = os_control::poll_privacy_sensors();
        let mut mic_attrs = BTreeMap::new();
        if !privacy.microphone_apps.is_empty() {
            mic_attrs.insert("active_apps".into(), serde_json::json!(privacy.microphone_apps));
        }
        states.push(EntityState {
            id: "microphone_in_use".into(),
            value: EntityValue::Bool(privacy.microphone_active),
            attributes: mic_attrs,
            updated_at: now,
        });

        let mut cam_attrs = BTreeMap::new();
        if !privacy.webcam_apps.is_empty() {
            cam_attrs.insert("active_apps".into(), serde_json::json!(privacy.webcam_apps));
        }
        states.push(EntityState {
            id: "webcam_in_use".into(),
            value: EntityValue::Bool(privacy.webcam_active),
            attributes: cam_attrs,
            updated_at: now,
        });

        // Audio & OS updates
        states.push(EntityState {
            id: "master_volume".into(),
            value: EntityValue::Float(self.master_volume),
            attributes: BTreeMap::new(),
            updated_at: now,
        });
        states.push(EntityState {
            id: "audio_mute".into(),
            value: EntityValue::Bool(self.audio_muted),
            attributes: BTreeMap::new(),
            updated_at: now,
        });
        states.push(EntityState {
            id: "audio_output".into(),
            value: EntityValue::Text(self.audio_output.clone()),
            attributes: BTreeMap::new(),
            updated_at: now,
        });
        states.push(EntityState {
            id: "os_updates".into(),
            value: EntityValue::Update(self.os_updates.clone()),
            attributes: BTreeMap::new(),
            updated_at: now,
        });

        states
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
            ..LightCapabilities::default()
        }),
        exposed: true,
    }
}

fn button_desc(id: &str, name: &str, icon: &str) -> EntityDescriptor {
    EntityDescriptor {
        id: id.into(),
        name: name.into(),
        platform: EntityPlatform::Button,
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
