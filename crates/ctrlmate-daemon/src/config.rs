use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ctrlmate_protocol::{
    AllowlistedScript, PollingProfile, RemotePermissions, SecurityMode, TemperatureUnit,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredClient {
    pub client_id: String,
    pub client_name: String,
    /// Hex-encoded SHA-256 digest of the issued bearer token (never stored in plaintext).
    pub token_hash: String,
    pub paired_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device_id: String,
    /// Hex-encoded 32-byte secret used for HMAC token signing.
    pub signing_secret_hex: String,
    pub security_mode: SecurityMode,
    pub polling_profile: PollingProfile,
    pub temperature_unit: TemperatureUnit,
    pub permissions: RemotePermissions,
    #[serde(default)]
    pub paired_clients: Vec<StoredClient>,
    #[serde(default)]
    pub scripts: Vec<AllowlistedScript>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let mut secret = [0u8; 32];
        rand::rng().fill_bytes(&mut secret);
        Self {
            device_id: Uuid::new_v4().to_string(),
            signing_secret_hex: hex::encode(secret),
            security_mode: SecurityMode::ZeroKernelSafe,
            polling_profile: PollingProfile::Intelligent,
            temperature_unit: TemperatureUnit::Celsius,
            permissions: RemotePermissions::default(),
            paired_clients: Vec::new(),
            scripts: Vec::new(),
        }
    }
}

impl DaemonConfig {
    pub fn resolve_data_dir(override_dir: Option<PathBuf>) -> Result<PathBuf> {
        let dir = if let Some(custom) = override_dir {
            custom
        } else if let Some(proj) = directories::ProjectDirs::from("org", "ctrlmate", "Ctrlmate") {
            proj.config_dir().to_path_buf()
        } else {
            PathBuf::from(".ctrlmate-data")
        };

        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create config dir {}", dir.display()))?;
        Ok(dir)
    }

    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("config.json");
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("failed reading {}", path.display()))?;
            let cfg: Self = serde_json::from_str(&raw)
                .with_context(|| format!("failed parsing {}", path.display()))?;
            Ok(cfg)
        } else {
            let cfg = Self::default();
            cfg.save(data_dir)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let path = data_dir.join("config.json");
        let tmp = data_dir.join("config.json.tmp");
        let body = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}
