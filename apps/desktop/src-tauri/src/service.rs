//! Service management (architecture contract sections 8 and 9) and agent autostart.
//!
//! * `service_status` compares the app version with the daemon version reported in the snapshot.
//! * On Windows the install / upgrade runs the installed `ctrlmated.exe --install-service` /
//!   `--upgrade-service` through an elevated `ShellExecute` (`runas`, UAC prompt).
//! * On Linux the package manager owns the service: the UI shows instructions.

use crate::error::{ErrorCode, IpcError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// No daemon answers (not installed, stopped, or not started yet).
    Offline,
    /// A daemon answers but is not the installed service (dev / per-user run).
    NotInstalled,
    /// The installed service has the same version as the app.
    Current,
    /// The installed service is older than the app: offer an upgrade.
    Outdated,
    /// The installed service is newer than the app.
    Newer,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub platform: &'static str,
    pub app_version: String,
    pub daemon_version: Option<String>,
    pub running_as_service: Option<bool>,
    pub agent_connected: Option<bool>,
    pub state: ServiceState,
    /// `ctrlmated` found next to the app and the platform can elevate it.
    pub installer_available: bool,
    pub agent_binary_available: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    Install,
    Upgrade,
}

// `Completed`, `Cancelled` and `Failed` are only produced by the Windows elevation path.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ElevatedStatus {
    Completed,
    /// The user dismissed the UAC prompt.
    Cancelled,
    Failed,
    /// Nothing was run: the platform expects a package manager (Linux).
    Manual,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElevatedOutcome {
    pub status: ElevatedStatus,
    pub exit_code: Option<i32>,
}

pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// Parses `1.2.3` / `1.2.3-rc1` into a comparable triple.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

pub fn classify(app: &str, snapshot_service: Option<&Value>) -> (ServiceState, Option<String>) {
    let Some(svc) = snapshot_service else {
        return (ServiceState::Offline, None);
    };
    let version = svc
        .get("version")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let as_service = svc
        .get("running_as_service")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !as_service {
        return (ServiceState::NotInstalled, version);
    }
    let state = match (
        version.as_deref().and_then(parse_version),
        parse_version(app),
    ) {
        (Some(d), Some(a)) if d < a => ServiceState::Outdated,
        (Some(d), Some(a)) if d > a => ServiceState::Newer,
        (Some(_), Some(_)) => ServiceState::Current,
        // Unparseable daemon version: assume it needs the upgrade rather than hiding the problem.
        _ => ServiceState::Outdated,
    };
    (state, version)
}

pub fn helper_binary(stem: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    };
    let p = dir.join(name);
    p.is_file().then_some(p)
}

pub fn build_status(app_version: &str, snapshot_service: Option<&Value>) -> ServiceStatus {
    let (state, daemon_version) = classify(app_version, snapshot_service);
    ServiceStatus {
        platform: platform(),
        app_version: app_version.to_string(),
        daemon_version,
        running_as_service: snapshot_service
            .and_then(|s| s.get("running_as_service"))
            .and_then(Value::as_bool),
        agent_connected: snapshot_service
            .and_then(|s| s.get("agent_connected"))
            .and_then(Value::as_bool),
        state,
        installer_available: cfg!(windows) && helper_binary("ctrlmated").is_some(),
        agent_binary_available: helper_binary("ctrlmate-agent").is_some(),
    }
}

/// Runs `ctrlmated --install-service` or `--upgrade-service` elevated (Windows) and waits for it.
pub async fn install_or_upgrade(action: ServiceAction) -> Result<ElevatedOutcome, IpcError> {
    if !cfg!(windows) {
        return Ok(ElevatedOutcome {
            status: ElevatedStatus::Manual,
            exit_code: None,
        });
    }
    let exe = helper_binary("ctrlmated").ok_or(IpcError::new(ErrorCode::BinaryNotFound))?;
    let flag = match action {
        ServiceAction::Install => "--install-service",
        ServiceAction::Upgrade => "--upgrade-service",
    };
    tauri::async_runtime::spawn_blocking(move || run_elevated(&exe, flag))
        .await
        .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))
}

#[cfg(windows)]
fn run_elevated(exe: &std::path::Path, arg: &str) -> ElevatedOutcome {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    const WAIT_MS: u32 = 180_000;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb = wide(std::ffi::OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    let params = wide(std::ffi::OsStr::new(arg));
    let dir = exe.parent().map(|p| wide(p.as_os_str()));

    // SAFETY: SHELLEXECUTEINFOW is plain data; every pointer stays valid for the call and the
    // process handle is closed before returning.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0);
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.lpDirectory = dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr());
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            let status = if GetLastError() == ERROR_CANCELLED {
                ElevatedStatus::Cancelled
            } else {
                ElevatedStatus::Failed
            };
            return ElevatedOutcome {
                status,
                exit_code: None,
            };
        }
        let handle = info.hProcess;
        if handle.is_null() {
            return ElevatedOutcome {
                status: ElevatedStatus::Failed,
                exit_code: None,
            };
        }
        let wait = WaitForSingleObject(handle, WAIT_MS);
        let mut code: u32 = 0;
        let got = wait == 0 && GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        if !got {
            return ElevatedOutcome {
                status: ElevatedStatus::Failed,
                exit_code: None,
            };
        }
        let code = i32::try_from(code).unwrap_or(-1);
        ElevatedOutcome {
            status: if code == 0 {
                ElevatedStatus::Completed
            } else {
                ElevatedStatus::Failed
            },
            exit_code: Some(code),
        }
    }
}

#[cfg(not(windows))]
fn run_elevated(_exe: &std::path::Path, _arg: &str) -> ElevatedOutcome {
    ElevatedOutcome {
        status: ElevatedStatus::Manual,
        exit_code: None,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AutostartOutcome {
    pub ok: bool,
    pub exit_code: Option<i32>,
}

/// Runs `ctrlmate-agent --install-autostart` (per-user, no elevation).
pub async fn install_agent_autostart() -> Result<AutostartOutcome, IpcError> {
    let exe = helper_binary("ctrlmate-agent").ok_or(IpcError::new(ErrorCode::BinaryNotFound))?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--install-autostart");
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let status = cmd
        .status()
        .await
        .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))?;
    Ok(AutostartOutcome {
        ok: status.success(),
        exit_code: status.code(),
    })
}

/// Starts `ctrlmate-agent` now, detached from this app (it keeps running if the UI quits).
/// The agent holds a single-instance lock, so this is a no-op when it already runs.
pub fn start_agent_detached() {
    let Some(exe) = helper_binary("ctrlmate-agent") else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    if let Ok(mut child) = cmd.spawn() {
        // Reap it when it exits so a second instance that bails out does not linger as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("0.6.0"), Some((0, 6, 0)));
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("2"), Some((2, 0, 0)));
        assert_eq!(parse_version("x"), None);
    }

    #[test]
    fn classification() {
        assert_eq!(classify("0.6.0", None).0, ServiceState::Offline);
        let dev = json!({"running_as_service": false, "version": "0.6.0"});
        assert_eq!(classify("0.6.0", Some(&dev)).0, ServiceState::NotInstalled);
        let old = json!({"running_as_service": true, "version": "0.5.2"});
        assert_eq!(classify("0.6.0", Some(&old)).0, ServiceState::Outdated);
        let same = json!({"running_as_service": true, "version": "0.6.0"});
        assert_eq!(classify("0.6.0", Some(&same)).0, ServiceState::Current);
        let newer = json!({"running_as_service": true, "version": "0.7.0"});
        assert_eq!(classify("0.6.0", Some(&newer)).0, ServiceState::Newer);
        let junk = json!({"running_as_service": true, "version": ""});
        assert_eq!(classify("0.6.0", Some(&junk)).0, ServiceState::Outdated);
    }
}
