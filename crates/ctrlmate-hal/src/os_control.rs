//! Cross-platform OS controls: Power, Displays, Privacy (Microphone/Webcam), Allowlisted Scripts.

use anyhow::{bail, Context, Result};
use ctrlmate_protocol::AllowlistedScript;
use std::process::Command;
use tracing::info;

#[derive(Debug, Clone, Default)]
pub struct PrivacyStatus {
    pub microphone_active: bool,
    pub microphone_apps: Vec<String>,
    pub webcam_active: bool,
    pub webcam_apps: Vec<String>,
}

/// Checks whether any application is currently using the microphone or webcam.
#[cfg(target_os = "windows")]
pub fn poll_privacy_sensors() -> PrivacyStatus {
    let mic_apps = active_windows_capability_apps("microphone");
    let cam_apps = active_windows_capability_apps("webcam");
    PrivacyStatus {
        microphone_active: !mic_apps.is_empty(),
        microphone_apps: mic_apps,
        webcam_active: !cam_apps.is_empty(),
        webcam_apps: cam_apps,
    }
}

/// Inspects `HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\<cap>`
/// for entries with `LastUsedTimeStart > 0` and `LastUsedTimeStop == 0`.
#[cfg(target_os = "windows")]
fn active_windows_capability_apps(capability: &str) -> Vec<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let base_path = format!(
        r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{capability}"
    );
    let Ok(base) = hkcu.open_subkey(&base_path) else {
        return Vec::new();
    };

    let mut active = Vec::new();
    collect_active_subkeys(&base, &mut active);

    if let Ok(non_packaged) = base.open_subkey("NonPackaged") {
        collect_active_subkeys(&non_packaged, &mut active);
    }

    active
}

#[cfg(target_os = "windows")]
fn collect_active_subkeys(parent: &winreg::RegKey, active: &mut Vec<String>) {
    for sub_name in parent.enum_keys().flatten() {
        if sub_name.eq_ignore_ascii_case("NonPackaged") {
            continue;
        }
        if let Ok(sub) = parent.open_subkey(&sub_name) {
            let start: u64 = sub.get_value("LastUsedTimeStart").unwrap_or(0);
            let stop: u64 = sub.get_value("LastUsedTimeStop").unwrap_or(1);
            if start > 0 && stop == 0 {
                let clean = sub_name
                    .rsplit('#')
                    .next()
                    .unwrap_or(&sub_name)
                    .to_string();
                active.push(clean);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub fn poll_privacy_sensors() -> PrivacyStatus {
    // Detect V4L2 direct webcam opens via /proc/*/fd -> /dev/video*
    let mut cam_apps = Vec::new();
    if let Ok(proc_dir) = std::fs::read_dir("/proc") {
        for entry in proc_dir.flatten() {
            let fname = entry.file_name();
            let Some(pid_str) = fname.to_str() else {
                continue;
            };
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let fd_dir = entry.path().join("fd");
            if let Ok(fds) = std::fs::read_dir(fd_dir) {
                for fd in fds.flatten() {
                    if let Ok(target) = std::fs::read_link(fd.path()) {
                        if target.to_string_lossy().starts_with("/dev/video") {
                            let comm = std::fs::read_to_string(entry.path().join("comm"))
                                .unwrap_or_else(|_| pid_str.to_string())
                                .trim()
                                .to_string();
                            if !cam_apps.contains(&comm) {
                                cam_apps.push(comm);
                            }
                        }
                    }
                }
            }
        }
    }

    PrivacyStatus {
        microphone_active: false,
        microphone_apps: Vec::new(),
        webcam_active: !cam_apps.is_empty(),
        webcam_apps: cam_apps,
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn poll_privacy_sensors() -> PrivacyStatus {
    PrivacyStatus::default()
}

/// Executes a system power / display action.
pub fn execute_power_action(action_id: &str) -> Result<String> {
    info!("Executing OS power action: {action_id}");

    #[cfg(target_os = "windows")]
    {
        match action_id {
            "power_shutdown" => {
                Command::new("shutdown")
                    .args(["/s", "/t", "3", "/c", "Ctrlmate: Shutdown requested"])
                    .spawn()
                    .context("failed to trigger shutdown")?;
                Ok("Shutdown scheduled (3s)".into())
            }
            "power_restart" => {
                Command::new("shutdown")
                    .args(["/r", "/t", "3", "/c", "Ctrlmate: Restart requested"])
                    .spawn()
                    .context("failed to trigger restart")?;
                Ok("Restart scheduled (3s)".into())
            }
            "power_sleep" => {
                unsafe {
                    windows::Win32::System::Power::SetSuspendState(false, false, false);
                }
                Ok("Entering sleep".into())
            }
            "power_hibernate" => {
                unsafe {
                    windows::Win32::System::Power::SetSuspendState(true, false, false);
                }
                Ok("Entering hibernate".into())
            }
            "power_lock" => {
                unsafe {
                    windows::Win32::System::Shutdown::LockWorkStation()
                        .context("LockWorkStation failed")?;
                }
                Ok("Session locked".into())
            }
            "display_off" => {
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{
                    SendMessageTimeoutW, HWND_BROADCAST, SC_MONITORPOWER, SMTO_ABORTIFHUNG,
                    WM_SYSCOMMAND,
                };
                unsafe {
                    SendMessageTimeoutW(
                        HWND_BROADCAST,
                        WM_SYSCOMMAND,
                        WPARAM(SC_MONITORPOWER as usize),
                        LPARAM(2),
                        SMTO_ABORTIFHUNG,
                        1000,
                        None,
                    );
                }
                Ok("Displays turned off".into())
            }
            _ => bail!("Unknown power action: {action_id}"),
        }
    }

    #[cfg(target_os = "linux")]
    {
        let args: &[&str] = match action_id {
            "power_shutdown" => &["poweroff"],
            "power_restart" => &["reboot"],
            "power_sleep" => &["suspend"],
            "power_hibernate" => &["hibernate"],
            "power_lock" => &["lock-session"],
            "display_off" => {
                let _ = Command::new("xset").args(["dpms", "force", "off"]).spawn();
                return Ok("Display off requested".into());
            }
            _ => bail!("Unknown power action: {action_id}"),
        };
        let bin = if action_id == "power_lock" {
            "loginctl"
        } else {
            "systemctl"
        };
        Command::new(bin)
            .args(args)
            .spawn()
            .with_context(|| format!("failed to execute {bin} {args:?}"))?;
        Ok(format!("Executed {action_id}"))
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        bail!("Unsupported OS for power action: {action_id}")
    }
}

/// Runs a strictly allowlisted custom script (never arbitrary remote shell commands).
pub fn execute_allowlisted_script(script: &AllowlistedScript) -> Result<String> {
    if !script.enabled {
        bail!("Script '{}' is disabled", script.name);
    }

    let mut cmd = Command::new(&script.executable);
    cmd.args(&script.args);
    if let Some(dir) = &script.working_dir {
        cmd.current_dir(dir);
    }

    cmd.spawn()
        .with_context(|| format!("failed to spawn allowlisted script '{}'", script.name))?;
    Ok(format!("Launched script '{}'", script.name))
}
