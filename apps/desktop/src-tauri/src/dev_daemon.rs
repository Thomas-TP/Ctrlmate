//! Debug builds only: an opt-in way to run a throw-away `ctrlmated --foreground` for UI work.
//! Release builds never spawn a daemon (architecture contract section 8: the service owns it).
//!
//! Enable with `CTRLMATE_DEV_SPAWN_DAEMON=1`; override the binary with `CTRLMATE_DEV_DAEMON_PATH`.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

pub const ENV_ENABLE: &str = "CTRLMATE_DEV_SPAWN_DAEMON";
pub const ENV_PATH: &str = "CTRLMATE_DEV_DAEMON_PATH";

pub fn enabled() -> bool {
    std::env::var(ENV_ENABLE).is_ok_and(|v| v == "1")
}

/// Data dir of the dev daemon (only when the opt-in is active).
pub fn data_dir() -> Option<PathBuf> {
    enabled().then(|| std::env::temp_dir().join("ctrlmate-dev-data"))
}

#[derive(Default)]
pub struct DevDaemon(Mutex<Option<Child>>);

impl DevDaemon {
    pub fn spawn_if_enabled(&self) {
        let Some(dir) = data_dir() else { return };
        let exe = std::env::var_os(ENV_PATH).map(PathBuf::from).or_else(|| {
            let me = std::env::current_exe().ok()?;
            let name = if cfg!(windows) {
                "ctrlmated.exe"
            } else {
                "ctrlmated"
            };
            Some(me.parent()?.join(name))
        });
        let Some(exe) = exe.filter(|p| p.is_file()) else {
            tracing::warn!("dev daemon requested but ctrlmated was not found");
            return;
        };
        let mut cmd = Command::new(exe);
        cmd.arg("--foreground")
            .arg("--data-dir")
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match cmd.spawn() {
            Ok(child) => {
                if let Ok(mut g) = self.0.lock() {
                    *g = Some(child);
                }
            }
            Err(e) => tracing::warn!("could not spawn dev daemon: {e}"),
        }
    }

    pub fn kill(&self) {
        if let Ok(mut g) = self.0.lock()
            && let Some(mut c) = g.take()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
