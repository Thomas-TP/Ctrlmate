//! IPC token discovery (architecture contract section 2): env `CTRLMATE_IPC_TOKEN`, per-user dir,
//! system dir. The token never leaves this process; the webview only sees [`TokenSource`].

use crate::error::ErrorCode;
use serde::Serialize;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub const TOKEN_FILE: &str = "ipc.token";
pub const ENV_TOKEN: &str = "CTRLMATE_IPC_TOKEN";
const TOKEN_HEX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Env,
    User,
    System,
    /// Debug builds only: the data dir of a daemon spawned by the app itself.
    Dev,
}

pub type Candidate = (TokenSource, Result<String, ErrorCode>);

fn is_valid_format(s: &str) -> bool {
    s.len() == TOKEN_HEX_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Per-user data dir, identical to the daemon's `datadir::user_dir`.
pub fn user_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("org", "ctrlmate", "Ctrlmate")
        .map(|p| p.config_dir().to_path_buf())
}

/// System data dir of the service.
pub fn system_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("ProgramData")
            .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
        Some(base.join("Ctrlmate"))
    }
    #[cfg(target_os = "linux")]
    {
        Some(PathBuf::from("/var/lib/ctrlmate"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

fn read_file(dir: &Path) -> Option<Result<String, ErrorCode>> {
    match std::fs::read_to_string(dir.join(TOKEN_FILE)) {
        Ok(s) => {
            let t = s.trim().to_string();
            Some(if is_valid_format(&t) {
                Ok(t)
            } else {
                Err(ErrorCode::TokenInvalid)
            })
        }
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            Some(Err(ErrorCode::TokenPermissionDenied))
        }
        Err(_) => Some(Err(ErrorCode::TokenNotFound)),
    }
}

/// Builds the candidate list from explicit inputs (testable). Sources that simply do not exist
/// are omitted; unreadable or malformed ones are kept with their error.
pub fn candidates_from(env: Option<&str>, dirs: &[(TokenSource, PathBuf)]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    if let Some(v) = env.map(str::trim).filter(|v| !v.is_empty()) {
        out.push((
            TokenSource::Env,
            if is_valid_format(v) {
                Ok(v.to_string())
            } else {
                Err(ErrorCode::TokenInvalid)
            },
        ));
    }
    for (source, dir) in dirs {
        if let Some(r) = read_file(dir) {
            out.push((*source, r));
        }
    }
    out
}

/// Picks the first usable token that was not already rejected by the daemon.
pub fn pick(
    candidates: &[Candidate],
    rejected: &HashSet<String>,
) -> Result<(TokenSource, String), ErrorCode> {
    let mut permission = false;
    let mut invalid = false;
    let mut was_rejected = false;
    for (source, r) in candidates {
        match r {
            Ok(t) if rejected.contains(t) => was_rejected = true,
            Ok(t) => return Ok((*source, t.clone())),
            Err(ErrorCode::TokenPermissionDenied) => permission = true,
            Err(_) => invalid = true,
        }
    }
    Err(if permission {
        ErrorCode::TokenPermissionDenied
    } else if was_rejected {
        ErrorCode::Unauthorized
    } else if invalid {
        ErrorCode::TokenInvalid
    } else {
        ErrorCode::TokenNotFound
    })
}

/// Directories scanned after the environment variable, in order.
pub fn search_dirs() -> Vec<(TokenSource, PathBuf)> {
    let mut dirs = Vec::new();
    #[cfg(debug_assertions)]
    if let Some(d) = crate::dev_daemon::data_dir() {
        dirs.push((TokenSource::Dev, d));
    }
    if let Some(d) = user_dir() {
        dirs.push((TokenSource::User, d));
    }
    if let Some(d) = system_dir() {
        dirs.push((TokenSource::System, d));
    }
    dirs
}

/// Fresh read of every source (the daemon may have rotated or created the token meanwhile).
pub fn candidates() -> Vec<Candidate> {
    candidates_from(std::env::var(ENV_TOKEN).ok().as_deref(), &search_dirs())
}

pub fn resolve(rejected: &HashSet<String>) -> Result<(TokenSource, String), ErrorCode> {
    pick(&candidates(), rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn tmp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ctrlmate-token-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap_or_default();
        d
    }

    #[test]
    fn env_wins_then_user_then_system() {
        let user = tmp("user");
        let system = tmp("system");
        std::fs::write(user.join(TOKEN_FILE), format!("{B}\n")).unwrap_or_default();
        std::fs::write(system.join(TOKEN_FILE), A).unwrap_or_default();
        let dirs = [(TokenSource::User, user), (TokenSource::System, system)];
        let c = candidates_from(Some(A), &dirs);
        assert_eq!(
            pick(&c, &HashSet::new()),
            Ok((TokenSource::Env, A.to_string()))
        );
        let c = candidates_from(None, &dirs);
        assert_eq!(
            pick(&c, &HashSet::new()),
            Ok((TokenSource::User, B.to_string()))
        );
        // The daemon rejected the per-user token: fall through to the system one.
        let rejected: HashSet<String> = [B.to_string()].into();
        assert_eq!(
            pick(&c, &rejected),
            Ok((TokenSource::System, A.to_string()))
        );
        let rejected: HashSet<String> = [A.to_string(), B.to_string()].into();
        assert_eq!(pick(&c, &rejected), Err(ErrorCode::Unauthorized));
    }

    #[test]
    fn missing_and_malformed_are_typed() {
        assert_eq!(pick(&[], &HashSet::new()), Err(ErrorCode::TokenNotFound));
        let c = candidates_from(Some("not-hex"), &[]);
        assert_eq!(pick(&c, &HashSet::new()), Err(ErrorCode::TokenInvalid));
        let d = tmp("empty");
        assert!(candidates_from(None, &[(TokenSource::User, d)]).is_empty());
    }

    #[test]
    fn permission_error_has_priority() {
        let c: Vec<Candidate> = vec![
            (TokenSource::User, Err(ErrorCode::TokenInvalid)),
            (TokenSource::System, Err(ErrorCode::TokenPermissionDenied)),
        ];
        assert_eq!(
            pick(&c, &HashSet::new()),
            Err(ErrorCode::TokenPermissionDenied)
        );
    }
}
