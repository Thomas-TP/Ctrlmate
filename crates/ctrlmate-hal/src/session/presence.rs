//! Lock state and input-idle time of the user session.
//!
//! * Windows: the workstation is locked while the *input desktop* cannot be opened (the secure
//!   Winlogon desktop is active); idle time from `GetLastInputInfo`.
//! * Linux: logind `LockedHint` / `IdleHint` of the caller's session (`loginctl`), and `xprintidle`
//!   on X11 for a real idle time.
//!
//! `None` = not measurable here.

pub fn is_locked() -> Option<bool> {
    imp::is_locked()
}

/// Seconds since the last keyboard / mouse input.
pub fn idle_seconds() -> Option<u64> {
    imp::idle_seconds()
}

/// `loginctl show-session -p Key --value` style text -> bool (`yes` / `no`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_yes_no(text: &str) -> Option<bool> {
    match text.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// Idle seconds from logind's `IdleSinceHint` (microseconds since the epoch, `0` = not idle).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn idle_from_since_hint(idle_hint: bool, since_usec: u64, now_usec: u64) -> u64 {
    if !idle_hint || since_usec == 0 {
        return 0;
    }
    now_usec.saturating_sub(since_usec) / 1_000_000
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, OpenInputDesktop,
    };
    use windows::Win32::System::SystemInformation::GetTickCount64;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    pub fn is_locked() -> Option<bool> {
        // SAFETY: the desktop handle, when opened, is closed right away.
        unsafe {
            match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) {
                Ok(h) => {
                    let _ = CloseDesktop(h);
                    Some(false)
                }
                // Access denied while the secure desktop (lock screen, UAC) has the input.
                Err(_) => Some(true),
            }
        }
    }

    pub fn idle_seconds() -> Option<u64> {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is initialised with its size.
        unsafe {
            if !GetLastInputInfo(&mut info).as_bool() {
                return None;
            }
            // dwTime is a 32-bit tick count: compare in the same 32-bit domain (wraps after 49 days).
            let now32 = GetTickCount64() as u32;
            Some(u64::from(now32.wrapping_sub(info.dwTime)) / 1000)
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{idle_from_since_hint, parse_yes_no};
    use crate::session::exec::run_ok;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const T: Duration = Duration::from_secs(2);

    fn session_prop(key: &str) -> Option<String> {
        let sid = std::env::var("XDG_SESSION_ID")
            .ok()
            .filter(|s| !s.is_empty());
        let arg = format!("-p{key}");
        let out = match &sid {
            Some(id) => run_ok("loginctl", &["show-session", id, &arg, "--value"], T),
            None => run_ok("loginctl", &["show-session", "self", &arg, "--value"], T),
        };
        out.ok().map(|s| s.trim().to_string())
    }

    pub fn is_locked() -> Option<bool> {
        parse_yes_no(&session_prop("LockedHint")?)
    }

    pub fn idle_seconds() -> Option<u64> {
        if super::super::foreground::is_x11()
            && let Ok(ms) = run_ok("xprintidle", &[], T)
            && let Ok(ms) = ms.trim().parse::<u64>()
        {
            return Some(ms / 1000);
        }
        let hint = parse_yes_no(&session_prop("IdleHint")?)?;
        let since: u64 = session_prop("IdleSinceHint")?.parse().ok()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_micros() as u64;
        Some(idle_from_since_hint(hint, since, now))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    pub fn is_locked() -> Option<bool> {
        None
    }
    pub fn idle_seconds() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logind_parsing() {
        assert_eq!(parse_yes_no("yes\n"), Some(true));
        assert_eq!(parse_yes_no("no"), Some(false));
        assert_eq!(parse_yes_no(""), None);
        assert_eq!(idle_from_since_hint(true, 1_000_000, 61_000_000), 60);
        assert_eq!(idle_from_since_hint(false, 1_000_000, 61_000_000), 0);
        assert_eq!(idle_from_since_hint(true, 0, 5), 0);
        // clock skew never underflows
        assert_eq!(idle_from_since_hint(true, 10, 5), 0);
    }

    #[cfg(windows)]
    #[test]
    fn windows_queries_answer_in_an_interactive_test_run() {
        // May be None / locked on a headless CI runner, but must not panic.
        let _ = is_locked();
        let _ = idle_seconds();
    }
}
