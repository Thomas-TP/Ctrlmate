//! Logging to the user state directory (`%LOCALAPPDATA%\Ctrlmate\logs\agent.log`,
//! `~/.local/state/ctrlmate/agent.log`). A file above 2 MiB is rotated once at start.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_subscriber::EnvFilter;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

pub fn log_dir() -> Option<PathBuf> {
    let p = directories::ProjectDirs::from("org", "ctrlmate", "Ctrlmate")?;
    Some(
        p.state_dir()
            .map_or_else(|| p.data_local_dir().join("logs"), Path::to_path_buf),
    )
}

/// Opens `dir/agent.log` for appending, rotating a big file to `agent.log.1` first.
pub fn open_log(dir: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("agent.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("agent.log.1"));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Installs the global subscriber; falls back to stderr when no log file can be opened.
pub fn init(default_level: &str) {
    let filter =
        EnvFilter::try_from_env("CTRLMATE_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));
    let file = log_dir().and_then(|d| open_log(&d).ok());
    let b = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    let _ = match file {
        Some(f) => b.with_writer(Mutex::new(f)).try_init(),
        None => b.try_init(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn big_logs_rotate_and_small_ones_append() {
        let d = tempfile::tempdir().unwrap();
        let mut f = open_log(d.path()).unwrap();
        f.write_all(b"one\n").unwrap();
        drop(f);
        let mut f = open_log(d.path()).unwrap();
        f.write_all(b"two\n").unwrap();
        drop(f);
        assert_eq!(
            std::fs::read_to_string(d.path().join("agent.log")).unwrap(),
            "one\ntwo\n"
        );
        let big = vec![b'x'; (MAX_LOG_BYTES + 10) as usize];
        std::fs::write(d.path().join("agent.log"), big).unwrap();
        drop(open_log(d.path()).unwrap());
        assert!(d.path().join("agent.log.1").exists());
        assert_eq!(
            std::fs::metadata(d.path().join("agent.log")).unwrap().len(),
            0
        );
    }
}
