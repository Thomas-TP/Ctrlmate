//! Lock the session, turn the displays off, show a notification: everything that must run *in* the
//! interactive session. Missing backends return [`SessionError::NotSupported`], never fake success.

use super::SessionError;

pub fn lock() -> Result<(), SessionError> {
    imp::lock()
}

pub fn display_off() -> Result<(), SessionError> {
    imp::display_off()
}

pub fn notify(title: &str, body: &str) -> Result<(), SessionError> {
    if title.chars().any(|c| c == '\0') || body.chars().any(|c| c == '\0') {
        return Err(SessionError::Invalid("NUL in notification text".into()));
    }
    imp::notify(title, body)
}

#[cfg(target_os = "windows")]
mod imp {
    use super::SessionError;
    use crate::session::exec::run_capture;
    use std::process::Command;
    use std::time::Duration;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Shutdown::LockWorkStation;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SC_MONITORPOWER, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SYSCOMMAND,
    };

    pub fn lock() -> Result<(), SessionError> {
        // SAFETY: no arguments; only valid in an interactive session, which the agent is.
        unsafe { LockWorkStation() }
            .map_err(|e| SessionError::Failed(format!("LockWorkStation: {e}")))
    }

    pub fn display_off() -> Result<(), SessionError> {
        // SAFETY: documented broadcast of SC_MONITORPOWER (2 = off).
        let r = unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SYSCOMMAND,
                WPARAM(SC_MONITORPOWER as usize),
                LPARAM(2),
                SMTO_ABORTIFHUNG,
                1000,
                None,
            )
        };
        if r.0 == 0 {
            Err(SessionError::Failed(
                "SC_MONITORPOWER broadcast failed or timed out".into(),
            ))
        } else {
            Ok(())
        }
    }

    /// The text travels in environment variables, never through the script text (no injection).
    const TOAST_PS: &str = r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$esc = { param($s) [System.Security.SecurityElement]::Escape($s) }
$t = & $esc $env:CTRLMATE_TOAST_TITLE
$m = & $esc $env:CTRLMATE_TOAST_BODY
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>$t</text><text>$m</text></binding></visual></toast>")
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
$appId = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast)
"#;

    pub fn notify(title: &str, body: &str) -> Result<(), SessionError> {
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            TOAST_PS,
        ])
        .env("CTRLMATE_TOAST_TITLE", title)
        .env("CTRLMATE_TOAST_BODY", body);
        match run_capture(c, Duration::from_secs(20), 2048) {
            Ok(r) if r.success() => Ok(()),
            Ok(r) => Err(SessionError::Failed(format!(
                "toast exit {:?}: {}",
                r.code,
                r.output.trim()
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(SessionError::NotSupported("powershell not found".into()))
            }
            Err(e) => Err(SessionError::Failed(e.to_string())),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::SessionError;
    use crate::session::exec::run_ok;
    use crate::session::foreground::is_x11;
    use std::io::ErrorKind;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(8);

    fn map(tool: &str, e: &std::io::Error) -> SessionError {
        if e.kind() == ErrorKind::NotFound {
            SessionError::NotSupported(format!("{tool} is not installed"))
        } else {
            SessionError::Failed(e.to_string())
        }
    }

    /// Runs in the user session: `loginctl lock-session` without an id locks the caller's session.
    pub fn lock() -> Result<(), SessionError> {
        run_ok("loginctl", &["lock-session"], T)
            .map(|_| ())
            .map_err(|e| map("loginctl", &e))
    }

    pub fn display_off() -> Result<(), SessionError> {
        if is_x11() {
            return run_ok("xset", &["dpms", "force", "off"], T)
                .map(|_| ())
                .map_err(|e| map("xset", &e));
        }
        // Wayland: wlroots compositors (sway, Hyprland, ...) through wlopm; others have no portable API.
        run_ok("wlopm", &["--off", "*"], T)
            .map(|_| ())
            .map_err(|e| {
                if e.kind() == ErrorKind::NotFound {
                    SessionError::NotSupported(
                        "no display-power tool for this Wayland compositor (wlopm)".into(),
                    )
                } else {
                    SessionError::Failed(e.to_string())
                }
            })
    }

    pub fn notify(title: &str, body: &str) -> Result<(), SessionError> {
        // `--` ends option parsing so a title that starts with `-` cannot become a flag.
        run_ok(
            "notify-send",
            &["--app-name=Ctrlmate", "--", title, body],
            T,
        )
        .map(|_| ())
        .map_err(|e| map("notify-send", &e))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::SessionError;
    fn ns() -> Result<(), SessionError> {
        Err(SessionError::NotSupported(
            "not supported on this OS".into(),
        ))
    }
    pub fn lock() -> Result<(), SessionError> {
        ns()
    }
    pub fn display_off() -> Result<(), SessionError> {
        ns()
    }
    pub fn notify(_: &str, _: &str) -> Result<(), SessionError> {
        ns()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_in_text_is_rejected_without_spawning() {
        assert!(matches!(notify("a\0b", "x"), Err(SessionError::Invalid(_))));
    }
}
