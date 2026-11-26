//! Data directory resolution and secret-file protection (architecture contract section 2).
//!
//! * Windows service: `%ProgramData%\Ctrlmate`, SYSTEM + Administrators only. `ipc.token` is the
//!   single exception (`Users` read) so the per-user desktop app and agent can talk to the daemon.
//! * Linux service: `/var/lib/ctrlmate`, dir `0750`, secrets `0600`, token `0640` group `ctrlmate`.
//! * Dev / per-user daemon: the per-user config directory, owner only.

use anyhow::{Context, Result};
use ctrlmate_protocol::DataDirKind;
use std::path::{Path, PathBuf};

/// How a file in the data directory is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Readable by the daemon account (and administrators) only.
    Secret,
    /// The IPC token: additionally readable by every local user in `System` data dirs.
    IpcToken,
}

#[derive(Debug, Clone)]
pub struct DataDir {
    pub path: PathBuf,
    pub kind: DataDirKind,
}

/// System-wide data directory of the service, when the platform has one.
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

/// Per-user data directory (dev / per-user daemon).
pub fn user_dir() -> PathBuf {
    directories::ProjectDirs::from("org", "ctrlmate", "Ctrlmate").map_or_else(
        || PathBuf::from(".ctrlmate-data"),
        |p| p.config_dir().to_path_buf(),
    )
}

impl DataDir {
    /// Resolves the data directory.
    ///
    /// * `--data-dir` wins (kind `Custom`, or `System` when it equals the system dir).
    /// * `STATE_DIRECTORY` (systemd `StateDirectory=`) or `service == true` selects the system dir.
    /// * Otherwise the per-user directory.
    pub fn resolve(override_dir: Option<PathBuf>, service: bool) -> Self {
        let sys = system_dir();
        if let Some(custom) = override_dir {
            let kind = if sys.as_deref() == Some(custom.as_path()) {
                DataDirKind::System
            } else {
                DataDirKind::Custom
            };
            return Self { path: custom, kind };
        }
        if let Some(state) = std::env::var_os("STATE_DIRECTORY")
            .and_then(|v| v.to_string_lossy().split(':').next().map(PathBuf::from))
            .filter(|p| !p.as_os_str().is_empty())
        {
            return Self {
                path: state,
                kind: DataDirKind::System,
            };
        }
        if service && let Some(s) = sys {
            return Self {
                path: s,
                kind: DataDirKind::System,
            };
        }
        Self {
            path: user_dir(),
            kind: DataDirKind::User,
        }
    }

    /// Directory a one-shot CLI command (`--rotate-ipc-token`) should act on: an explicit
    /// `--data-dir`, else the system directory when a token already lives there (an installed
    /// service), else the per-user directory.
    pub fn resolve_for_cli(override_dir: Option<PathBuf>) -> Self {
        if override_dir.is_none()
            && let Some(sys) = system_dir()
            && sys.join(crate::ipc_token::TOKEN_FILE).exists()
        {
            return Self {
                path: sys,
                kind: DataDirKind::System,
            };
        }
        Self::resolve(override_dir, false)
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Creates the directory and applies the directory protection of its kind.
    pub fn prepare(&self) -> Result<()> {
        std::fs::create_dir_all(&self.path)
            .with_context(|| format!("failed to create data dir {}", self.path.display()))?;
        platform::protect_dir(&self.path, self.kind)
            .with_context(|| format!("failed to protect data dir {}", self.path.display()))
    }

    /// Atomically writes `bytes` to `name`: the temporary file is created inside the protected
    /// directory (never world readable), protected, then renamed over the target.
    pub fn write_protected(&self, name: &str, bytes: &[u8], protection: Protection) -> Result<()> {
        let target = self.file(name);
        let tmp = self.file(&format!("{name}.tmp"));
        platform::create_private_file(&tmp, bytes)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        if let Err(e) = platform::protect_file(&tmp, self.kind, protection) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("failed to protect {}", tmp.display()));
        }
        std::fs::rename(&tmp, &target)
            .with_context(|| format!("failed to replace {}", target.display()))
    }

    /// Re-applies protection to an existing file (used after reading legacy files).
    pub fn protect_existing(&self, name: &str, protection: Protection) -> Result<()> {
        platform::protect_file(&self.file(name), self.kind, protection)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{DataDirKind, Protection};
    use anyhow::{Context, Result, bail};
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    pub const SID_SYSTEM: &str = "*S-1-5-18";
    pub const SID_ADMINS: &str = "*S-1-5-32-544";
    pub const SID_USERS: &str = "*S-1-5-32-545";

    fn icacls(path: &Path, args: &[&str]) -> Result<()> {
        let out = Command::new("icacls")
            .arg(path)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .context("failed to run icacls")?;
        if !out.status.success() {
            bail!(
                "icacls {} failed: {}{}",
                path.display(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    /// SID of the account running this process, as `*S-1-...` (locale independent).
    fn current_sid() -> Result<String> {
        static SID: OnceLock<Option<String>> = OnceLock::new();
        let sid = SID.get_or_init(|| {
            let out = Command::new("whoami")
                .args(["/user", "/fo", "csv", "/nh"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let sid = text
                .trim()
                .rsplit(',')
                .next()?
                .trim()
                .trim_matches('"')
                .to_string();
            sid.starts_with("S-1-").then(|| format!("*{sid}"))
        });
        sid.clone()
            .context("could not determine the current user SID")
    }

    pub fn create_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
        // The data dir ACL is inherited (owner/admin only for every kind), so the file is private
        // from the first byte on.
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn protect_dir(path: &Path, kind: DataDirKind) -> Result<()> {
        match kind {
            DataDirKind::System => {
                let full_sys = format!("{SID_SYSTEM}:(OI)(CI)F");
                let full_adm = format!("{SID_ADMINS}:(OI)(CI)F");
                // Users get traverse on the directory itself only (not inherited): enough to reach
                // ipc.token, not enough to list or read anything else.
                let traverse = format!("{SID_USERS}:(X)");
                icacls(
                    path,
                    &[
                        "/inheritance:r",
                        "/grant:r",
                        &full_sys,
                        &full_adm,
                        &traverse,
                    ],
                )
            }
            DataDirKind::User | DataDirKind::Custom => {
                let me = current_sid()?;
                let full_me = format!("{me}:(OI)(CI)F");
                let full_sys = format!("{SID_SYSTEM}:(OI)(CI)F");
                let full_adm = format!("{SID_ADMINS}:(OI)(CI)F");
                icacls(
                    path,
                    &["/inheritance:r", "/grant:r", &full_me, &full_sys, &full_adm],
                )
            }
        }
    }

    pub fn protect_file(path: &Path, kind: DataDirKind, protection: Protection) -> Result<()> {
        match (kind, protection) {
            (DataDirKind::System, Protection::IpcToken) => {
                let sys = format!("{SID_SYSTEM}:F");
                let adm = format!("{SID_ADMINS}:F");
                let usr = format!("{SID_USERS}:R");
                icacls(path, &["/inheritance:r", "/grant:r", &sys, &adm, &usr])
            }
            (DataDirKind::System, Protection::Secret) => {
                let sys = format!("{SID_SYSTEM}:F");
                let adm = format!("{SID_ADMINS}:F");
                icacls(path, &["/inheritance:r", "/grant:r", &sys, &adm])
            }
            (_, _) => {
                let me = format!("{}:F", current_sid()?);
                let sys = format!("{SID_SYSTEM}:F");
                icacls(path, &["/inheritance:r", "/grant:r", &me, &sys])
            }
        }
    }

    /// Returns the SDDL string of a path (`icacls /save`), for tests and diagnostics.
    #[cfg(test)]
    pub fn sddl(path: &Path) -> Result<String> {
        let out = std::env::temp_dir().join(format!("ctrlmate-sddl-{}.txt", rand::random::<u64>()));
        let status = Command::new("icacls")
            .arg(path)
            .arg("/save")
            .arg(&out)
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if !status.status.success() {
            bail!("icacls /save failed");
        }
        let raw = std::fs::read(&out)?;
        let _ = std::fs::remove_file(&out);
        let (pairs, _) = raw[2..].as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

#[cfg(unix)]
mod platform {
    use super::{DataDirKind, Protection};
    use anyhow::Result;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    /// Group id of `ctrlmate` from /etc/group, when the group exists.
    fn ctrlmate_gid() -> Option<u32> {
        let text = std::fs::read_to_string("/etc/group").ok()?;
        text.lines().find_map(|l| {
            let mut it = l.split(':');
            (it.next()? == "ctrlmate").then(|| it.nth(1)?.parse().ok())?
        })
    }

    fn chgrp(path: &Path) {
        if let Some(gid) = ctrlmate_gid() {
            let _ = std::os::unix::fs::chown(path, None, Some(gid));
        }
    }

    pub fn create_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
        let _ = std::fs::remove_file(path);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    }

    pub fn protect_dir(path: &Path, kind: DataDirKind) -> Result<()> {
        if kind == DataDirKind::System {
            chgrp(path);
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o750))?;
        } else {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    pub fn protect_file(path: &Path, kind: DataDirKind, protection: Protection) -> Result<()> {
        let mode = if kind == DataDirKind::System && protection == Protection::IpcToken {
            chgrp(path);
            0o640
        } else {
            0o600
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        Ok(())
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
mod platform {
    use super::{DataDirKind, Protection};
    use anyhow::Result;
    use std::path::Path;

    pub fn create_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
        std::fs::write(path, bytes)?;
        Ok(())
    }
    pub fn protect_dir(_: &Path, _: DataDirKind) -> Result<()> {
        Ok(())
    }
    pub fn protect_file(_: &Path, _: DataDirKind, _: Protection) -> Result<()> {
        Ok(())
    }
}

/// Reads a file fully, mapping "does not exist" to `None`.
pub fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(dir: &Path) -> DataDir {
        DataDir {
            path: dir.to_path_buf(),
            kind: DataDirKind::Custom,
        }
    }

    #[test]
    fn override_is_custom_and_wins() {
        let d = DataDir::resolve(Some(PathBuf::from("somewhere")), true);
        assert_eq!(d.kind, DataDirKind::Custom);
        assert_eq!(d.path, PathBuf::from("somewhere"));
    }

    #[test]
    fn override_equal_to_system_dir_is_system() {
        if let Some(sys) = system_dir() {
            assert_eq!(DataDir::resolve(Some(sys), false).kind, DataDirKind::System);
        }
    }

    #[test]
    fn write_protected_replaces_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = custom(tmp.path());
        dd.prepare().unwrap();
        dd.write_protected("a.bin", b"one", Protection::Secret)
            .unwrap();
        dd.write_protected("a.bin", b"two", Protection::Secret)
            .unwrap();
        assert_eq!(std::fs::read(dd.file("a.bin")).unwrap(), b"two");
        assert!(!dd.file("a.bin.tmp").exists());
        assert_eq!(read_optional(&dd.file("missing")).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_modes_are_tight() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dd = custom(tmp.path());
        dd.prepare().unwrap();
        dd.write_protected("s", b"x", Protection::Secret).unwrap();
        dd.write_protected("t", b"x", Protection::IpcToken).unwrap();
        let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dd.path.clone()), 0o700);
        assert_eq!(mode(dd.file("s")), 0o600);
        // Custom dirs never widen the token.
        assert_eq!(mode(dd.file("t")), 0o600);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_acls_are_protected_and_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = custom(tmp.path());
        dd.prepare().unwrap();
        dd.write_protected("s", b"x", Protection::Secret).unwrap();
        for p in [dd.path.clone(), dd.file("s")] {
            let sddl = platform::sddl(&p).unwrap();
            assert!(
                sddl.contains("D:P"),
                "inheritance not removed on {p:?}: {sddl}"
            );
            // No Everyone (WD), Users (BU), Authenticated Users (AU).
            for bad in [";;;WD)", ";;;BU)", ";;;AU)"] {
                assert!(!sddl.contains(bad), "{bad} present on {p:?}: {sddl}");
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_system_token_acl_grants_users_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("ipc.token");
        std::fs::write(&f, b"x").unwrap();
        platform::protect_file(&f, DataDirKind::System, Protection::IpcToken).unwrap();
        let sddl = platform::sddl(&f).unwrap();
        assert!(sddl.contains("D:P"), "{sddl}");
        assert!(sddl.contains(";;FR;;;BU)"), "Users read missing: {sddl}");
        assert!(sddl.contains(";;FA;;;SY)"), "SYSTEM full missing: {sddl}");
        assert!(
            sddl.contains(";;FA;;;BA)"),
            "Administrators full missing: {sddl}"
        );
        assert!(!sddl.contains(";;FA;;;BU)"), "{sddl}");
    }
}
