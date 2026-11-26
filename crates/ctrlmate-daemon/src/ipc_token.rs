//! IPC bearer token: 32 random bytes (hex), generated on first start, rotatable, compared in
//! constant time. The token file lives in the data dir (see `datadir`).

use crate::datadir::{DataDir, Protection, read_optional};
use anyhow::Result;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use subtle::ConstantTimeEq;
use tokio::sync::watch;

pub const TOKEN_FILE: &str = "ipc.token";
const TOKEN_HEX_LEN: usize = 64;

/// Generates a fresh token (256 bits of OS randomness, lowercase hex).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn is_valid_format(s: &str) -> bool {
    s.len() == TOKEN_HEX_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Constant-time equality that does not leak the length either (both sides are hashed first).
pub fn tokens_equal(a: &str, b: &str) -> bool {
    let ha = Sha256::digest(a.as_bytes());
    let hb = Sha256::digest(b.as_bytes());
    bool::from(ha.ct_eq(&hb))
}

fn read_token(dd: &DataDir) -> Result<Option<String>> {
    Ok(read_optional(&dd.file(TOKEN_FILE))?
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .filter(|t| is_valid_format(t)))
}

/// Writes `token` with the protection of the data dir kind.
fn write_token(dd: &DataDir, token: &str) -> Result<()> {
    dd.write_protected(TOKEN_FILE, token.as_bytes(), Protection::IpcToken)
}

/// Returns the existing token or creates one. An unreadable / malformed file is replaced.
pub fn load_or_create(dd: &DataDir) -> Result<String> {
    dd.prepare()?;
    if let Some(t) = read_token(dd)? {
        // Heal permissions of tokens written by older versions.
        let _ = dd.protect_existing(TOKEN_FILE, Protection::IpcToken);
        return Ok(t);
    }
    let token = generate_token();
    write_token(dd, &token)?;
    Ok(token)
}

/// Replaces the token. Running daemons notice the new file within a few seconds
/// (see [`TokenStore::spawn_watcher`]); clients re-read the file on authentication failure.
pub fn rotate(dd: &DataDir) -> Result<String> {
    dd.prepare()?;
    let token = generate_token();
    write_token(dd, &token)?;
    Ok(token)
}

/// In-memory copy of the token plus a generation counter that changes on rotation.
pub struct TokenStore {
    dir: DataDir,
    current: RwLock<Arc<str>>,
    generation_tx: watch::Sender<u64>,
}

impl TokenStore {
    pub fn load(dd: &DataDir) -> Result<Arc<Self>> {
        let token = load_or_create(dd)?;
        Ok(Arc::new(Self::with_token(dd.clone(), &token)))
    }

    fn with_token(dir: DataDir, token: &str) -> Self {
        let (generation_tx, _) = watch::channel(0);
        Self {
            dir,
            current: RwLock::new(Arc::from(token)),
            generation_tx,
        }
    }

    /// Constant-time check of a presented token.
    pub fn check(&self, presented: &str) -> bool {
        let cur = self
            .current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        tokens_equal(&cur, presented)
    }

    /// Subscribe to rotation events; the value increases on every change.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation_tx.subscribe()
    }

    /// Re-reads the file; returns `true` when the token changed.
    pub fn reload(&self) -> bool {
        let Ok(Some(new)) = read_token(&self.dir) else {
            return false;
        };
        let changed = {
            let mut guard = self
                .current
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens_equal(&guard, &new) {
                false
            } else {
                *guard = Arc::from(new.as_str());
                true
            }
        };
        if changed {
            self.generation_tx.send_modify(|g| *g += 1);
        }
        changed
    }

    /// Polls the token file every 2 s so `ctrlmated --rotate-ipc-token` takes effect on a running
    /// daemon without a restart; rotation closes every live IPC WebSocket.
    pub fn spawn_watcher(self: &Arc<Self>) {
        let store = Arc::clone(self);
        tokio::spawn(async move {
            let path: PathBuf = store.dir.file(TOKEN_FILE);
            let mut last: Option<SystemTime> = None;
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tick.tick().await;
                let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if mtime != last {
                    last = mtime;
                    if store.reload() {
                        tracing::info!("IPC token rotated; closing live IPC connections");
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctrlmate_protocol::DataDirKind;

    fn dd(dir: &std::path::Path) -> DataDir {
        DataDir {
            path: dir.to_path_buf(),
            kind: DataDirKind::Custom,
        }
    }

    #[test]
    fn generated_tokens_are_256_bit_hex_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(is_valid_format(&a));
        assert_ne!(a, b);
    }

    #[test]
    fn constant_time_compare_semantics() {
        let t = generate_token();
        assert!(tokens_equal(&t, &t.clone()));
        assert!(!tokens_equal(&t, &generate_token()));
        assert!(!tokens_equal(&t, ""));
        assert!(!tokens_equal(&t, &t[..63]));
    }

    #[test]
    fn load_creates_once_and_rotate_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        let t1 = load_or_create(&d).unwrap();
        assert_eq!(load_or_create(&d).unwrap(), t1);
        let t2 = rotate(&d).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(load_or_create(&d).unwrap(), t2);
    }

    #[test]
    fn malformed_token_file_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        std::fs::write(d.file(TOKEN_FILE), "short").unwrap();
        let t = load_or_create(&d).unwrap();
        assert!(is_valid_format(&t));
    }

    #[test]
    fn store_reload_detects_rotation_and_bumps_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        let store = TokenStore::load(&d).unwrap();
        let old = std::fs::read_to_string(d.file(TOKEN_FILE)).unwrap();
        assert!(store.check(&old));
        let rx = store.subscribe();
        assert!(!store.reload());
        let new = rotate(&d).unwrap();
        assert!(store.reload());
        assert!(store.check(&new));
        assert!(!store.check(&old));
        assert_eq!(*rx.borrow(), 1);
    }
}
