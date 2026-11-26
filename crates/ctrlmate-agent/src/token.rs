//! IPC token lookup (architecture contract section 2): environment `CTRLMATE_IPC_TOKEN`, then the
//! per-user data directory, then the system data directory.

use std::path::{Path, PathBuf};

pub const TOKEN_ENV: &str = "CTRLMATE_IPC_TOKEN";
pub const TOKEN_FILE: &str = "ipc.token";

/// Where a token was found (for diagnostics; never log the token itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    File(PathBuf),
}

/// Candidate data directories in lookup order: per-user first, then the system directory.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = directories::ProjectDirs::from("org", "ctrlmate", "Ctrlmate") {
        v.push(p.config_dir().to_path_buf());
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("ProgramData")
            .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
        v.push(base.join("Ctrlmate"));
    }
    #[cfg(target_os = "linux")]
    v.push(PathBuf::from("/var/lib/ctrlmate"));
    v
}

fn clean(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && t.len() <= 256 && !t.contains(char::is_whitespace)).then(|| t.to_string())
}

/// Pure lookup: `env` first, then each directory's `ipc.token`.
pub fn find_token_in(env: Option<&str>, dirs: &[PathBuf]) -> Option<(String, TokenSource)> {
    if let Some(t) = env.and_then(clean) {
        return Some((t, TokenSource::Env));
    }
    dirs.iter().find_map(|d| read_file(&d.join(TOKEN_FILE)))
}

fn read_file(p: &Path) -> Option<(String, TokenSource)> {
    let raw = std::fs::read_to_string(p).ok()?;
    clean(&raw).map(|t| (t, TokenSource::File(p.to_path_buf())))
}

/// Looks the token up in the real environment and directories.
pub fn find_token() -> Option<(String, TokenSource)> {
    find_token_in(std::env::var(TOKEN_ENV).ok().as_deref(), &data_dirs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_order_is_env_then_user_then_system() {
        let user = tempfile::tempdir().unwrap();
        let sys = tempfile::tempdir().unwrap();
        std::fs::write(sys.path().join(TOKEN_FILE), "system-token\n").unwrap();
        let dirs = vec![user.path().to_path_buf(), sys.path().to_path_buf()];
        // Only the system dir has one.
        let (t, src) = find_token_in(None, &dirs).unwrap();
        assert_eq!(t, "system-token");
        assert_eq!(src, TokenSource::File(sys.path().join(TOKEN_FILE)));
        // The per-user token wins over the system one.
        std::fs::write(user.path().join(TOKEN_FILE), "  user-token \r\n").unwrap();
        assert_eq!(find_token_in(None, &dirs).unwrap().0, "user-token");
        // The environment wins over both.
        let (t, src) = find_token_in(Some("env-token"), &dirs).unwrap();
        assert_eq!((t.as_str(), src), ("env-token", TokenSource::Env));
    }

    #[test]
    fn empty_or_malformed_tokens_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(TOKEN_FILE), "   \n").unwrap();
        let dirs = vec![d.path().to_path_buf()];
        assert!(find_token_in(None, &dirs).is_none());
        assert!(find_token_in(Some(""), &dirs).is_none());
        assert!(find_token_in(Some("has space"), &dirs).is_none());
        // An empty env var falls through to the files.
        std::fs::write(d.path().join(TOKEN_FILE), "abc").unwrap();
        assert_eq!(find_token_in(Some(" "), &dirs).unwrap().0, "abc");
    }
}
