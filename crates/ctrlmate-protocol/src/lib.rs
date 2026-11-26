//! Shared strongly-typed wire & IPC protocol for Ctrlmate.
//!
//! Designed natively for Home Assistant 2026.11 (Parent PC + Child Devices,
//! `has_entity_name = true`, canonical °C telemetry with native HA °C/°F
//! conversion, RGB + Kelvin lighting, cooling presets, OS updates & control).

mod cooling;
mod result;
mod session;
mod status;

pub use cooling::*;
pub use result::*;
pub use session::*;
pub use status::*;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current wire protocol version spoken between `ctrlmated` and Home Assistant.
pub const PROTOCOL_VERSION: u16 = 1;

/// Default HTTPS/WSS port exposed on the LAN for Home Assistant.
pub const DEFAULT_HA_PORT: u16 = 9487;

/// Default local-only IPC port (loopback) for the desktop UI when Named Pipe / Unix socket is bypassed in dev.
pub const DEFAULT_IPC_PORT: u16 = 9488;

/// mDNS / Zeroconf service type discovered natively by Home Assistant 2026.11.
pub const MDNS_SERVICE_TYPE: &str = "_ctrlmate._tcp.local.";

/// Supported Kelvin range for PC lighting (matches Philips Hue & Govee warm-to-daylight range).
pub const MIN_COLOR_TEMP_KELVIN: u16 = 2000;
pub const MAX_COLOR_TEMP_KELVIN: u16 = 6500;

/// Hardware security / kernel isolation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    /// Zero kernel drivers loaded. 100% compatible with Riot Vanguard, FACEIT, EAC, Ricochet.
    #[default]
    ZeroKernelSafe,
    /// Optional Windows deep hardware extension (PawnIO / LHM) for Super I/O & SMBus RAM.
    DeepHardwarePawnIo,
}

/// Publication profile toward Home Assistant / MQTT. It sets the minimum interval between
/// two *numeric* updates of the same entity and the heartbeat (forced resend); boolean, text,
/// light and update entities are always pushed immediately on change.
///
/// | profile | min interval (gaming / active / locked-idle) | heartbeat |
/// |---|---|---|
/// | `intelligent` | 2 s / 5 s / 30 s | 60 s / 60 s / 120 s |
/// | `eco` | 15 s | 120 s |
/// | `balanced` | 5 s | 60 s |
/// | `realtime` | 1 s | 30 s |
///
/// The daemon's hardware *sampling* rate is separate: it is chosen by activity tier and
/// never slower than 5 s because the cooling watchdog needs fresh temperatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PollingProfile {
    /// Adaptive deadband + cadence that follows the activity tier.
    #[default]
    Intelligent,
    /// Low-traffic mode.
    Eco,
    /// Fixed 5 s cadence with deadband.
    Balanced,
    /// High-frequency telemetry (1 s).
    Realtime,
}

/// Temperature display preference in the desktop UI (protocol always sends canonical °C).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

/// Granular remote permission switches enforced by `ctrlmated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePermissions {
    pub telemetry: bool,
    pub lighting: bool,
    pub cooling: bool,
    pub audio_and_media: bool,
    pub power_control: bool,
    pub os_updates: bool,
    pub notifications: bool,
    pub custom_scripts: bool,
}

impl Default for RemotePermissions {
    fn default() -> Self {
        Self {
            telemetry: true,
            lighting: true,
            cooling: true,
            audio_and_media: true,
            power_control: true,
            os_updates: true,
            notifications: true,
            custom_scripts: false, // Explicit opt-in for security
        }
    }
}

/// Child device category modeled directly as Home Assistant 2026.11 Child Devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCategory {
    System,
    Cpu,
    Gpu,
    Motherboard,
    Memory,
    Storage,
    Cooler,
    Lighting,
    Audio,
}

/// Metadata describing the parent PC host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDeviceInfo {
    pub device_id: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub os_name: String,
    pub os_version: String,
    pub sw_version: String,
    /// Primary network interface MAC address (for Home Assistant Wake-on-LAN).
    pub mac_address: Option<String>,
    pub security_mode: SecurityMode,
}

/// Metadata describing a hardware component exposed as a Home Assistant Child Device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildDeviceInfo {
    /// Stable child identifier (e.g. `gpu_0`, `cpu_0`, `storage_nvme0`, `cooler_kraken`).
    pub id: String,
    pub name: String,
    pub category: DeviceCategory,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub hw_version: Option<String>,
}

/// Home Assistant entity domain / platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityPlatform {
    Sensor,
    BinarySensor,
    Light,
    Select,
    Button,
    Number,
    Switch,
    Update,
}

/// Entity category matching Home Assistant's `EntityCategory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityCategory {
    Config,
    Diagnostic,
}

/// Sensor state class matching Home Assistant's `SensorStateClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateClass {
    Measurement,
    Total,
    TotalIncreasing,
}

/// Static declaration of an entity exposed by Ctrlmate to Home Assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDescriptor {
    /// Stable entity key unique within this PC (e.g. `gpu_0_temperature`, `light_global`).
    pub id: String,
    /// Clean functional name (`has_entity_name = true` — e.g. `"Temperature"`, `"Load"`).
    pub name: String,
    pub platform: EntityPlatform,
    /// Optional child device id; `None` attaches the entity directly to the parent PC device.
    pub child_device_id: Option<String>,
    pub device_class: Option<String>,
    pub state_class: Option<StateClass>,
    pub unit_of_measurement: Option<String>,
    pub icon: Option<String>,
    pub entity_category: Option<EntityCategory>,
    pub suggested_display_precision: Option<u8>,
    /// Allowed options for `select` entities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Numeric bounds for `number` entities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_range: Option<NumberRange>,
    /// Lighting capabilities for `light` entities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_caps: Option<LightCapabilities>,
    /// Whether this entity is enabled for publication to Home Assistant.
    #[serde(default = "default_true")]
    pub exposed: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NumberRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightCapabilities {
    pub supports_rgb: bool,
    pub supports_color_temp: bool,
    pub min_color_temp_kelvin: u16,
    pub max_color_temp_kelvin: u16,
    pub supports_transition: bool,
    pub is_master_zone: bool,
    pub led_count: u16,
}

impl Default for LightCapabilities {
    fn default() -> Self {
        Self {
            supports_rgb: true,
            supports_color_temp: true,
            min_color_temp_kelvin: MIN_COLOR_TEMP_KELVIN,
            max_color_temp_kelvin: MAX_COLOR_TEMP_KELVIN,
            supports_transition: true,
            is_master_zone: false,
            led_count: 1,
        }
    }
}

/// Active color mode for a lighting entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightColorMode {
    Rgb,
    ColorTemp,
}

/// RGB triple (0–255 per channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Full state of a lighting zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightState {
    pub is_on: bool,
    /// Brightness 0–255 (Home Assistant native scale).
    pub brightness: u8,
    pub color_mode: LightColorMode,
    pub rgb: RgbColor,
    /// Color temperature in Kelvin (`2000..=6500`).
    pub color_temp_kelvin: u16,
}

impl Default for LightState {
    fn default() -> Self {
        Self {
            is_on: true,
            brightness: 204, // 80%
            color_mode: LightColorMode::ColorTemp,
            rgb: RgbColor {
                r: 255,
                g: 224,
                b: 189,
            },
            color_temp_kelvin: 4000,
        }
    }
}

/// State of the OS Update entity (`update.os_updates`) as produced by the hardware layer.
/// The honest, UI-facing view is [`UpdatesInfo`] in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OsUpdateState {
    pub installed_version: String,
    pub latest_version: String,
    pub pending_count: u32,
    pub pending_titles: Vec<String>,
    pub in_progress: bool,
    pub reboot_required: bool,
    pub last_checked: Option<DateTime<Utc>>,
}

/// User-selectable cooling preset (Home Assistant `select` + desktop UI).
/// `full_speed` is not a preset any more: it is the emergency state (`CoolingState::Failsafe`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CoolingPreset {
    /// Balanced curve; switches to `gaming` automatically while the agent reports a game.
    #[default]
    Auto,
    Silent,
    Balanced,
    Performance,
    Gaming,
}

impl CoolingPreset {
    pub const fn as_ha_option(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Silent => "silent",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
            Self::Gaming => "gaming",
        }
    }

    pub fn from_ha_option(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "silent" => Some(Self::Silent),
            "balanced" => Some(Self::Balanced),
            "performance" => Some(Self::Performance),
            "gaming" => Some(Self::Gaming),
            _ => None,
        }
    }

    pub const fn all() -> [Self; 5] {
        [
            Self::Auto,
            Self::Silent,
            Self::Balanced,
            Self::Performance,
            Self::Gaming,
        ]
    }

    pub fn all_ha_options() -> Vec<String> {
        Self::all()
            .iter()
            .map(|p| p.as_ha_option().to_string())
            .collect()
    }
}

/// Polymorphic live state value of an entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EntityValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Text(String),
    Light(LightState),
    /// Honest OS update status (see [`UpdatesInfo`]); there is no fabricated `latest_version`.
    Update(UpdatesInfo),
    Unavailable,
}

/// State payload for a single entity, optionally carrying structured HA attributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub id: String,
    pub value: EntityValue,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, serde_json::Value>,
    pub updated_at: DateTime<Utc>,
}

/// Detected conflicting vendor software (e.g. GCC, NZXT CAM, Armoury Crate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftwareConflict {
    pub id: String,
    pub name: String,
    pub category: String,
    pub running: bool,
    pub impact: String,
}

/// Detected USB HID device relevant for native lighting or AIO cooling control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedHidDevice {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: String,
    pub product: String,
    pub driver_hint: String,
    pub path: String,
}

/// Hardware status & transport diagnostics for a detected RGB controller or LED component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbDeviceStatus {
    pub id: String,
    pub name: String,
    pub zone_id: String,
    pub transport: String,
    pub controllable: bool,
    pub reason: String,
}

/// Optional secondary Home Assistant 2026.11 MQTT Device + Child Device Discovery transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MqttConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub use_tls: bool,
    pub discovery_prefix: String,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "homeassistant.local".into(),
            port: 1883,
            username: None,
            password: None,
            use_tls: false,
            discovery_prefix: "homeassistant".into(),
        }
    }
}

/// Allowlisted custom user script exposed as a Home Assistant button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowlistedScript {
    pub id: String,
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    pub enabled: bool,
}

/// Full manifest sent from `ctrlmated` to Home Assistant on connect & on topology change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceManifest {
    pub protocol_version: u16,
    pub host: HostDeviceInfo,
    pub child_devices: Vec<ChildDeviceInfo>,
    pub entities: Vec<EntityDescriptor>,
}

/// Commands initiated by Home Assistant (or the local Desktop UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ControlCommand {
    SetLight {
        entity_id: String,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    },
    SetCoolingPreset {
        entity_id: String,
        preset: CoolingPreset,
    },
    SetSelect {
        entity_id: String,
        option: String,
    },
    SetNumber {
        entity_id: String,
        value: f64,
    },
    SetSwitch {
        entity_id: String,
        state: bool,
    },
    PressButton {
        entity_id: String,
    },
    CheckOsUpdates,
    InstallOsUpdates,
    SendNotification {
        title: String,
        message: String,
    },
}

/// Top-level message envelope over the Home Assistant WebSocket (`wss://`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Hello {
        protocol_version: u16,
        device_id: String,
        hostname: String,
        sw_version: String,
        cert_fingerprint: String,
    },
    Auth {
        token: String,
        client_name: String,
    },
    AuthOk {
        client_id: String,
    },
    AuthError {
        reason: String,
    },
    Manifest(DeviceManifest),
    StateBatch {
        states: Vec<EntityState>,
    },
    Command {
        request_id: String,
        command: ControlCommand,
    },
    CommandResult {
        request_id: String,
        ok: bool,
        /// Stable [`ResultCode`] string. Older peers omit it.
        #[serde(default)]
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Ping {
        ts: i64,
    },
    Pong {
        ts: i64,
    },
}

/// Pairing API request (`POST /api/v1/pair`) using the temporary 6-digit PIN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub pin: String,
    pub client_name: String,
}

/// Pairing API response issuing a signed persistent token & TLS cert fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    pub client_id: String,
    pub token: String,
    pub device_id: String,
    pub hostname: String,
    pub mac_address: Option<String>,
    pub cert_fingerprint: String,
}

/// Active pairing session status visible in the desktop UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingSessionInfo {
    pub active: bool,
    pub pin: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cert_fingerprint: String,
    pub listen_port: u16,
}

/// Record of a paired Home Assistant instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedClientInfo {
    pub client_id: String,
    pub client_name: String,
    pub paired_at: DateTime<Utc>,
    /// At least one live connection.
    pub connected: bool,
    /// Number of live connections (each has its own connection id inside the daemon).
    #[serde(default)]
    pub connections: u32,
}

/// Live snapshot returned to the desktop UI over local IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonSnapshot {
    pub manifest: DeviceManifest,
    pub states: Vec<EntityState>,
    pub conflicts: Vec<SoftwareConflict>,
    pub hid_devices: Vec<DetectedHidDevice>,
    pub pairing: PairingSessionInfo,
    pub paired_clients: Vec<PairedClientInfo>,
    pub permissions: RemotePermissions,
    pub polling_profile: PollingProfile,
    pub temperature_unit: TemperatureUnit,
    pub scripts: Vec<AllowlistedScript>,
    pub thermal_failsafe_active: bool,
    #[serde(default)]
    pub rgb_backends: Vec<String>,
    #[serde(default)]
    pub rgb_devices: Vec<RgbDeviceStatus>,
    #[serde(default)]
    pub deep_bridge_active: bool,
    #[serde(default)]
    pub mqtt: MqttConfig,
    #[serde(default)]
    pub mqtt_connected: bool,
    /// The MQTT password is never sent back; this tells whether one is stored.
    #[serde(default)]
    pub mqtt_password_set: bool,
    /// Requested cooling preset (the effective one is in `cooling_policy`).
    #[serde(default)]
    pub cooling_preset: CoolingPreset,
    #[serde(default)]
    pub cooling_policy: CoolingPolicy,
    /// Availability of expected sensors, keyed by entity id.
    #[serde(default)]
    pub sensor_status: BTreeMap<String, SensorStatus>,
    /// Latest interactive-session state (from the agent or the in-process fallback);
    /// `None` when no fresh state exists.
    #[serde(default)]
    pub session: Option<SessionState>,
    #[serde(default)]
    pub service: ServiceInfo,
    #[serde(default)]
    pub updates: UpdatesInfo,
    /// OS update scans include optional updates, drivers and feature upgrades.
    #[serde(default)]
    pub include_optional_updates: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_message_roundtrip() {
        let msg = WireMessage::Command {
            request_id: "req-1".into(),
            command: ControlCommand::SetLight {
                entity_id: "light_global".into(),
                is_on: Some(true),
                brightness: Some(180),
                rgb: None,
                color_temp_kelvin: Some(2700),
                transition_ms: Some(1500),
            },
        };

        let encoded = serde_json::to_string(&msg).expect("serialize wire message");
        let decoded: WireMessage =
            serde_json::from_str(&encoded).expect("deserialize wire message");
        assert_eq!(msg, decoded);
    }

    #[test]
    fn command_result_wire_is_additive() {
        // Old shape (no `code`) still parses; new shape carries the code.
        let old: WireMessage =
            serde_json::from_str(r#"{"type":"command_result","request_id":"1","ok":true}"#)
                .unwrap();
        assert!(matches!(old, WireMessage::CommandResult { ref code, .. } if code.is_empty()));
        let new = WireMessage::CommandResult {
            request_id: "1".into(),
            ok: false,
            code: "no_agent".into(),
            message: None,
        };
        let v = serde_json::to_value(new).unwrap();
        assert_eq!(v["code"], "no_agent");
        assert_eq!(v["ok"], false);
        assert!(v.get("message").is_none());
    }

    #[test]
    fn full_speed_is_not_a_user_preset() {
        assert_eq!(CoolingPreset::from_ha_option("full_speed"), None);
        assert!(serde_json::from_str::<CoolingPreset>("\"full_speed\"").is_err());
        assert_eq!(
            CoolingPreset::from_ha_option("gaming"),
            Some(CoolingPreset::Gaming)
        );
    }

    #[test]
    fn legacy_snapshot_json_still_parses() {
        // Everything added in v0.6 is optional on the wire.
        let snap = sample_snapshot();
        let mut v = serde_json::to_value(&snap).unwrap();
        let obj = v.as_object_mut().unwrap();
        for k in [
            "mqtt_password_set",
            "cooling_preset",
            "cooling_policy",
            "sensor_status",
            "session",
            "service",
            "updates",
            "include_optional_updates",
        ] {
            obj.remove(k);
        }
        let back: DaemonSnapshot = serde_json::from_value(v).unwrap();
        assert_eq!(back.cooling_policy, CoolingPolicy::builtin());
        assert!(back.session.is_none());
    }

    fn sample_snapshot() -> DaemonSnapshot {
        DaemonSnapshot {
            manifest: DeviceManifest {
                protocol_version: PROTOCOL_VERSION,
                host: HostDeviceInfo {
                    device_id: "d".into(),
                    name: "pc".into(),
                    manufacturer: "m".into(),
                    model: "x".into(),
                    os_name: "Windows".into(),
                    os_version: "11".into(),
                    sw_version: "0.6.0".into(),
                    mac_address: None,
                    security_mode: SecurityMode::ZeroKernelSafe,
                },
                child_devices: vec![],
                entities: vec![],
            },
            states: vec![],
            conflicts: vec![],
            hid_devices: vec![],
            pairing: PairingSessionInfo {
                active: false,
                pin: None,
                expires_at: None,
                cert_fingerprint: "f".into(),
                listen_port: DEFAULT_HA_PORT,
            },
            paired_clients: vec![],
            permissions: RemotePermissions::default(),
            polling_profile: PollingProfile::default(),
            temperature_unit: TemperatureUnit::default(),
            scripts: vec![],
            thermal_failsafe_active: false,
            rgb_backends: vec![],
            rgb_devices: vec![],
            deep_bridge_active: false,
            mqtt: MqttConfig::default(),
            mqtt_connected: false,
            mqtt_password_set: false,
            cooling_preset: CoolingPreset::Auto,
            cooling_policy: CoolingPolicy::builtin(),
            sensor_status: BTreeMap::new(),
            session: None,
            service: ServiceInfo::default(),
            updates: UpdatesInfo::default(),
            include_optional_updates: false,
        }
    }

    #[test]
    fn ipc_event_snapshot_is_tagged() {
        let ev = IpcEvent::Snapshot(Box::new(sample_snapshot()));
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "snapshot");
        assert_eq!(v["permissions"]["telemetry"], true);
        assert_eq!(serde_json::from_value::<IpcEvent>(v).unwrap(), ev);
        assert_eq!(
            serde_json::to_value(IpcEvent::AuthOk).unwrap(),
            serde_json::json!({"type":"auth_ok"})
        );
    }

    #[test]
    fn cooling_preset_roundtrip() {
        for option in CoolingPreset::all_ha_options() {
            let parsed = CoolingPreset::from_ha_option(&option).expect("valid preset");
            assert_eq!(parsed.as_ha_option(), option);
        }
    }
}
