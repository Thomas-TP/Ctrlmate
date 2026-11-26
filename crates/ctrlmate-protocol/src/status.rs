//! Honest sensors, service / updates status and IPC-only types.

use crate::{AllowlistedScript, DaemonSnapshot, MqttConfig, PollingProfile, RemotePermissions};
use crate::{SecurityMode, TemperatureUnit};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Why a sensor has no value (architecture contract section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorReason {
    /// The hardware does not exist on this machine.
    NotPresent,
    /// Needs the optional deep-hardware (PawnIO / LHM) mode; impossible without a driver.
    RequiresDeepHardware,
    /// Deliberately stopped because an anti-cheat is running.
    AntiCheatActive,
    /// The sensor exists but reading it failed.
    Error,
    /// Access denied for the daemon account.
    Permission,
}

/// Availability of one telemetry entity. Present in `DaemonSnapshot::sensor_status`
/// for every entity the daemon knows it *should* normally have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorStatus {
    pub available: bool,
    /// Set iff `available == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<SensorReason>,
}

impl SensorStatus {
    pub const fn available() -> Self {
        Self {
            available: true,
            reason: None,
        }
    }

    pub const fn missing(reason: SensorReason) -> Self {
        Self {
            available: false,
            reason: Some(reason),
        }
    }
}

/// Kind of session the daemon runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    /// Windows service / systemd unit: no interactive session of its own.
    #[default]
    Service,
    /// Dev / foreground run inside a user session.
    Interactive,
}

/// Where the daemon keeps its data (and therefore which identity clients talk to).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DataDirKind {
    /// `%ProgramData%\Ctrlmate` or `/var/lib/ctrlmate`.
    System,
    /// Per-user config directory (dev / per-user daemon).
    #[default]
    User,
    /// Explicit `--data-dir`.
    Custom,
}

/// Facts about the running daemon that the desktop app needs to offer install / upgrade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ServiceInfo {
    pub running_as_service: bool,
    /// Daemon version (`CARGO_PKG_VERSION`).
    pub version: String,
    pub protocol_version: u16,
    pub session_type: SessionType,
    /// A `ctrlmate-agent` is connected and its state is fresh.
    pub agent_connected: bool,
    /// The daemon runs session features itself (dev mode, no agent needed).
    #[serde(default)]
    pub in_process_session: bool,
    pub data_dir_kind: DataDirKind,
    /// Version reported by the connected agent in its `hello`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    /// OS session id the agent runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<u32>,
}

/// One pending OS update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpdate {
    pub title: String,
    /// Knowledge-base id (`KB5034441`) when the title carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kb: Option<String>,
}

/// Honest OS update status. `pending_count` is `None` until a scan actually succeeded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdatesInfo {
    /// A scan completed at least once since the daemon started.
    pub checked: bool,
    pub last_checked: Option<DateTime<Utc>>,
    pub pending_count: Option<u32>,
    #[serde(default)]
    pub pending: Vec<PendingUpdate>,
    /// Titles of the pending updates, in the same order as `pending`.
    #[serde(default)]
    pub titles: Vec<String>,
    /// Flat list of KB ids, convenient for HA attributes.
    #[serde(default)]
    pub kb_ids: Vec<String>,
    pub reboot_required: bool,
    pub in_progress: bool,
    /// Result code of the last failed scan / install (`unavailable`, `internal`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub installed_version: String,
}

impl From<&crate::OsUpdateState> for UpdatesInfo {
    /// Honest view of the hardware layer's update state: nothing is reported as pending or
    /// counted until a scan has actually completed.
    fn from(u: &crate::OsUpdateState) -> Self {
        let checked = u.last_checked.is_some();
        let pending: Vec<PendingUpdate> = if checked {
            u.pending_titles
                .iter()
                .map(|t| PendingUpdate {
                    title: t.clone(),
                    kb: extract_kb_id(t),
                })
                .collect()
        } else {
            vec![]
        };
        Self {
            checked,
            last_checked: u.last_checked,
            pending_count: checked.then_some(u.pending_count),
            titles: pending.iter().map(|p| p.title.clone()).collect(),
            kb_ids: pending.iter().filter_map(|p| p.kb.clone()).collect(),
            pending,
            reboot_required: u.reboot_required,
            in_progress: u.in_progress,
            error: None,
            installed_version: u.installed_version.clone(),
        }
    }
}

/// Extracts a `KB1234567` id from an update title, if any.
pub fn extract_kb_id(title: &str) -> Option<String> {
    let bytes = title.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i].eq_ignore_ascii_case(&b'K')
            && bytes[i + 1].eq_ignore_ascii_case(&b'B')
            && bytes[i + 2].is_ascii_digit()
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let end = bytes[i + 2..]
                .iter()
                .position(|b| !b.is_ascii_digit())
                .map_or(bytes.len(), |p| i + 2 + p);
            return Some(format!("KB{}", &title[i + 2..end]));
        }
        i += 1;
    }
    None
}

/// Body of `PUT /ipc/settings`. Every field is optional; absent fields are unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SettingsUpdate {
    #[serde(default)]
    pub permissions: Option<RemotePermissions>,
    #[serde(default)]
    pub polling_profile: Option<PollingProfile>,
    #[serde(default)]
    pub temperature_unit: Option<TemperatureUnit>,
    #[serde(default)]
    pub security_mode: Option<SecurityMode>,
    /// Replaces the whole script allowlist (validated; IPC only, never over Home Assistant).
    #[serde(default)]
    pub scripts: Option<Vec<AllowlistedScript>>,
    /// `mqtt.password == None` keeps the stored password, `Some("")` clears it.
    #[serde(default)]
    pub mqtt: Option<MqttConfig>,
    /// Watchdog thresholds; values are clamped to their safe ranges (CPU critical 85-98 C) and take
    /// effect on the next cooling tick without a restart.
    #[serde(default)]
    pub thermal: Option<crate::ThermalThresholds>,
    /// Include optional updates, drivers and feature upgrades in OS update scans (default: security
    /// and quality updates only).
    #[serde(default)]
    pub include_optional_updates: Option<bool>,
}

/// Server to client events on `/ipc/ws`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcEvent {
    AuthOk,
    Snapshot(Box<DaemonSnapshot>),
}

/// Public (unauthenticated) `GET /api/v1/info` body. Without a valid paired bearer token only
/// the first five fields are present: no hostname and no MAC address leak to strangers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryInfo {
    pub protocol_version: u16,
    pub device_id: String,
    pub sw_version: String,
    pub cert_fingerprint: String,
    pub pairing_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kb_extraction() {
        assert_eq!(
            extract_kb_id("2026-11 Cumulative Update (KB5050009) for Windows 11"),
            Some("KB5050009".into())
        );
        assert_eq!(
            extract_kb_id("Security Intelligence Update kb2267602"),
            Some("KB2267602".into())
        );
        assert_eq!(extract_kb_id("Backblaze Update"), None);
        assert_eq!(extract_kb_id("NOTKB123"), None);
    }

    #[test]
    fn update_value_is_honest_on_the_wire() {
        let fresh = crate::OsUpdateState::default();
        let info = UpdatesInfo::from(&fresh);
        assert!(!info.checked);
        assert_eq!(info.pending_count, None);
        let v = serde_json::to_value(crate::EntityValue::Update(info)).unwrap();
        assert_eq!(v["kind"], "update");
        assert!(v["value"]["pending_count"].is_null());
        assert!(v["value"].get("latest_version").is_none());
        let scanned = crate::OsUpdateState {
            pending_count: 1,
            pending_titles: vec!["Update (KB5050009)".into()],
            last_checked: Some(chrono::Utc::now()),
            ..fresh
        };
        let info = UpdatesInfo::from(&scanned);
        assert_eq!(info.pending_count, Some(1));
        assert_eq!(info.kb_ids, vec!["KB5050009".to_string()]);
        assert_eq!(info.titles.len(), 1);
    }

    #[test]
    fn unavailable_value_wire_shape() {
        assert_eq!(
            serde_json::to_value(crate::EntityValue::Unavailable).unwrap(),
            serde_json::json!({"kind": "unavailable"})
        );
    }

    #[test]
    fn sensor_status_wire() {
        let v = serde_json::to_value(SensorStatus::missing(SensorReason::RequiresDeepHardware))
            .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"available": false, "reason": "requires_deep_hardware"})
        );
        let v = serde_json::to_value(SensorStatus::available()).unwrap();
        assert_eq!(v, serde_json::json!({"available": true}));
    }

    #[test]
    fn discovery_info_minimal_has_no_identity_leak() {
        let info = DiscoveryInfo {
            protocol_version: 1,
            device_id: "d".into(),
            sw_version: "0.6.0".into(),
            cert_fingerprint: "f".into(),
            pairing_active: false,
            hostname: None,
            mac_address: None,
        };
        let v = serde_json::to_value(info).unwrap();
        assert!(v.get("hostname").is_none());
        assert!(v.get("mac_address").is_none());
    }

    #[test]
    fn settings_update_accepts_partial_bodies() {
        let s: SettingsUpdate =
            serde_json::from_str(r#"{"temperature_unit":"fahrenheit"}"#).unwrap();
        assert_eq!(s.temperature_unit, Some(TemperatureUnit::Fahrenheit));
        assert!(s.scripts.is_none());
    }
}
