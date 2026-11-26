//! Persistent daemon configuration (`config.json` in the data dir, protected as a secret).

use crate::datadir::{DataDir, Protection};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ctrlmate_protocol::{
    AllowlistedScript, CoolingPreset, MqttConfig, PollingProfile, RemotePermissions, SecurityMode,
    TemperatureUnit, ThermalThresholds,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredClient {
    pub client_id: String,
    pub client_name: String,
    /// Hex SHA-256 of the issued bearer token. Tokens are 192 random bits, so a plain
    /// SHA-256 is sufficient (no password-hashing KDF needed) and the plaintext is never stored.
    pub token_hash: String,
    pub paired_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device_id: String,
    pub security_mode: SecurityMode,
    pub polling_profile: PollingProfile,
    pub temperature_unit: TemperatureUnit,
    pub permissions: RemotePermissions,
    /// Requested cooling preset, restored on start.
    #[serde(default)]
    pub cooling_preset: CoolingPreset,
    #[serde(default)]
    pub paired_clients: Vec<StoredClient>,
    #[serde(default)]
    pub scripts: Vec<AllowlistedScript>,
    #[serde(default)]
    pub mqtt: MqttConfig,
    /// Thermal watchdog thresholds (clamped; CPU critical 85-98 C, default 93).
    #[serde(default)]
    pub thermal: ThermalThresholds,
    /// OS update scans include optional updates, drivers and feature upgrades.
    #[serde(default)]
    pub include_optional_updates: bool,
    /// LibreHardwareMonitor control ids declared as chassis fans for the deep-hardware bridge.
    #[serde(default)]
    pub chassis_fan_ids: Vec<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            device_id: Uuid::new_v4().to_string(),
            security_mode: SecurityMode::ZeroKernelSafe,
            polling_profile: PollingProfile::Intelligent,
            temperature_unit: TemperatureUnit::Celsius,
            permissions: RemotePermissions::default(),
            cooling_preset: CoolingPreset::Auto,
            paired_clients: Vec::new(),
            scripts: Vec::new(),
            mqtt: MqttConfig::default(),
            thermal: ThermalThresholds::default(),
            include_optional_updates: false,
            chassis_fan_ids: Vec::new(),
        }
    }
}

impl DaemonConfig {
    /// Loads `config.json`, creating it with fresh defaults on first start. A file that exists
    /// but does not parse is an error (never silently replaced: it holds the paired clients).
    pub fn load_or_create(dd: &DataDir) -> Result<Self> {
        dd.prepare()?;
        let path = dd.file(CONFIG_FILE);
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("failed reading {}", path.display()))?;
            let mut cfg: Self = serde_json::from_str(&raw)
                .with_context(|| format!("failed parsing {}", path.display()))?;
            cfg.thermal = ctrlmate_hal::sanitize_thresholds(cfg.thermal);
            // Legacy files were written without protection.
            let _ = dd.protect_existing(CONFIG_FILE, Protection::Secret);
            Ok(cfg)
        } else {
            let cfg = Self::default();
            cfg.save(dd)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, dd: &DataDir) -> Result<()> {
        let body = serde_json::to_string_pretty(self)?;
        dd.write_protected(CONFIG_FILE, body.as_bytes(), Protection::Secret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctrlmate_protocol::DataDirKind;

    fn dd(p: &std::path::Path) -> DataDir {
        DataDir {
            path: p.to_path_buf(),
            kind: DataDirKind::Custom,
        }
    }

    #[test]
    fn create_save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        let mut c = DaemonConfig::load_or_create(&d).unwrap();
        c.cooling_preset = CoolingPreset::Gaming;
        c.save(&d).unwrap();
        let again = DaemonConfig::load_or_create(&d).unwrap();
        assert_eq!(again.device_id, c.device_id);
        assert_eq!(again.cooling_preset, CoolingPreset::Gaming);
    }

    #[test]
    fn legacy_v05_config_loads_and_drops_signing_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        let legacy = serde_json::json!({
            "device_id": "abc",
            "signing_secret_hex": "00".repeat(32),
            "security_mode": "zero_kernel_safe",
            "polling_profile": "intelligent",
            "temperature_unit": "celsius",
            "permissions": RemotePermissions::default(),
            "paired_clients": [],
        });
        std::fs::write(d.file(CONFIG_FILE), legacy.to_string()).unwrap();
        let c = DaemonConfig::load_or_create(&d).unwrap();
        assert_eq!(c.device_id, "abc");
        c.save(&d).unwrap();
        let raw = std::fs::read_to_string(d.file(CONFIG_FILE)).unwrap();
        assert!(!raw.contains("signing_secret"));
    }

    #[test]
    fn corrupt_config_is_an_error_not_a_reset() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        std::fs::write(d.file(CONFIG_FILE), "{ not json").unwrap();
        assert!(DaemonConfig::load_or_create(&d).is_err());
        assert_eq!(
            std::fs::read_to_string(d.file(CONFIG_FILE)).unwrap(),
            "{ not json"
        );
    }
}
