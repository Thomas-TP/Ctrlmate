//! Tiny app-local settings that the Rust side must know before the webview loads
//! (`start_hidden`, `close_to_tray`). Everything else lives in the daemon or in webview storage.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE: &str = "app-settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub close_to_tray: bool,
    pub start_hidden: bool,
    /// Start Ctrlmate (hidden in the tray) when the user signs in to the OS.
    pub launch_at_login: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            start_hidden: false,
            launch_at_login: true,
        }
    }
}

pub fn load(dir: &Path) -> AppSettings {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, s: &AppSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path: PathBuf = dir.join(FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default())?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let dir =
            std::env::temp_dir().join(format!("ctrlmate-settings-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), AppSettings::default());
        let s = AppSettings {
            close_to_tray: false,
            start_hidden: true,
            launch_at_login: false,
        };
        assert!(save(&dir, &s).is_ok());
        assert_eq!(load(&dir), s);
        // Partial / garbage files fall back to defaults field by field.
        std::fs::write(dir.join(FILE), r#"{"start_hidden":true}"#).unwrap_or_default();
        assert_eq!(
            load(&dir),
            AppSettings {
                close_to_tray: true,
                start_hidden: true,
                launch_at_login: true,
            }
        );
        std::fs::write(dir.join(FILE), "nope").unwrap_or_default();
        assert_eq!(load(&dir), AppSettings::default());
    }
}
