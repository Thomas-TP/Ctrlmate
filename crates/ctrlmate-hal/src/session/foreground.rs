//! Foreground application and gaming-mode detection.
//!
//! Gaming mode is **not** "a maximised window". The decision ([`is_gaming`]) combines
//! * the kind of fullscreen ([`Fullscreen`]): `Exclusive` (Direct3D exclusive, reported by the shell via
//!   `SHQueryUserNotificationState`) or `Borderless` (a caption-less window whose DWM extended frame
//!   bounds cover the whole monitor; a normal maximised window keeps its caption and stops at the
//!   work area),
//! * a deny list (shell, browsers, players, IDEs, remote desktop, office),
//! * allow hints (the executable lives in a game-library folder),
//! * for borderless windows, the GPU load (>= 40 %), because a caption-less full-monitor window can be
//!   a video player or a presentation.
//!
//! Linux: X11 sessions use `xdotool` + `xprop` (`_NET_WM_STATE_FULLSCREEN`); Wayland has no portable
//! "focused window" API, so there the foreground app is `None` and gaming mode is `false` (honest).

/// How the foreground window covers its monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fullscreen {
    #[default]
    No,
    /// Caption-less window covering the whole monitor.
    Borderless,
    /// Exclusive (D3D) fullscreen as reported by the shell.
    Exclusive,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Foreground {
    /// File name of the executable (`game.exe`), as the OS reports it.
    pub process_name: String,
    pub exe_path: Option<String>,
    pub window_title: Option<String>,
    pub fullscreen: Fullscreen,
}

/// Inputs of the gaming decision.
#[derive(Debug, Clone, Copy)]
pub struct GamingInput<'a> {
    pub process_name: &'a str,
    pub exe_path: Option<&'a str>,
    pub fullscreen: Fullscreen,
    pub gpu_load_pct: Option<u32>,
}

/// Processes that are never a game even when fullscreen (lower-case, no extension).
const DENY: &[&str] = &[
    // shell
    "explorer",
    "searchhost",
    "shellexperiencehost",
    "startmenuexperiencehost",
    "lockapp",
    "applicationframehost",
    "dwm",
    "textinputhost",
    "systemsettings",
    "taskmgr",
    // browsers
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "arc",
    "zen",
    "chromium",
    // players
    "vlc",
    "mpv",
    "mpc-hc",
    "mpc-hc64",
    "potplayer",
    "potplayermini64",
    "wmplayer",
    "spotify",
    "netflix",
    "disneyplus",
    // work
    "code",
    "devenv",
    "idea64",
    "pycharm64",
    "rider64",
    "powerpnt",
    "winword",
    "excel",
    "outlook",
    "teams",
    "ms-teams",
    "zoom",
    "slack",
    "discord",
    "obs64",
    "obs",
    "mstsc",
    "vmconnect",
    "windowsterminal",
    "wt",
    "cmd",
    "powershell",
    "pwsh",
    "conhost",
    "ctrlmate-desktop",
    "ctrlmate-agent",
    "anydesk",
    "teamviewer",
    "parsecd",
    // linux
    "gnome-shell",
    "plasmashell",
    "kwin_x11",
    "kwin_wayland",
    "firefox-esr",
    "mpv-wrapper",
];

/// Folder fragments of game libraries (lower-case, forward slashes).
const GAME_PATH_HINTS: &[&str] = &[
    "/steamapps/common/",
    "/epic games/",
    "/riot games/",
    "/gog galaxy/games/",
    "/gog games/",
    "/ubisoft game launcher/games/",
    "/ea games/",
    "/origin games/",
    "/xboxgames/",
    "/battle.net/",
    "/rockstar games/",
    "/heroic/",
    "/lutris/",
    "/.local/share/steam/steamapps/",
];

/// Minimum GPU load for a borderless window to count as a game.
pub const BORDERLESS_GPU_THRESHOLD: u32 = 40;

fn normalize_process(name: &str) -> String {
    let l = name.trim().to_ascii_lowercase();
    l.strip_suffix(".exe").unwrap_or(&l).to_string()
}

/// The gaming decision; pure, see the module documentation.
pub fn is_gaming(i: GamingInput<'_>) -> bool {
    let proc = normalize_process(i.process_name);
    if proc.is_empty() || DENY.contains(&proc.as_str()) {
        return false;
    }
    if i.fullscreen == Fullscreen::No {
        return false;
    }
    let path = i
        .exe_path
        .map(|p| p.replace('\\', "/").to_ascii_lowercase());
    if path
        .as_deref()
        .is_some_and(|p| GAME_PATH_HINTS.iter().any(|h| p.contains(h)))
    {
        return true;
    }
    match i.fullscreen {
        Fullscreen::Exclusive => true,
        Fullscreen::Borderless => i
            .gpu_load_pct
            .is_some_and(|l| l >= BORDERLESS_GPU_THRESHOLD),
        Fullscreen::No => false,
    }
}

/// `true` in an X11 (not Wayland) session.
#[cfg(target_os = "linux")]
pub(crate) fn is_x11() -> bool {
    imp::is_x11()
}

/// Reads the foreground window; `None` when there is none or it cannot be determined.
pub fn detect() -> Option<Foreground> {
    imp::detect()
}

/// Current GPU utilisation (NVML), `None` when no NVIDIA GPU / NVML. Initialised lazily.
pub struct GpuLoadProbe {
    nvml: Option<nvml_wrapper::Nvml>,
    tried: bool,
}

impl Default for GpuLoadProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuLoadProbe {
    pub fn new() -> Self {
        Self {
            nvml: None,
            tried: false,
        }
    }

    /// Highest utilisation over all NVIDIA GPUs.
    pub fn load_pct(&mut self) -> Option<u32> {
        if !self.tried {
            self.tried = true;
            self.nvml = nvml_wrapper::Nvml::init().ok();
        }
        let nvml = self.nvml.as_ref()?;
        let n = nvml.device_count().ok()?;
        (0..n)
            .filter_map(|i| nvml.device_by_index(i).ok()?.utilization_rates().ok())
            .map(|u| u.gpu)
            .max()
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{Foreground, Fullscreen};
    use std::path::Path;
    use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Shell::{
        QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowTextW,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_CAPTION,
    };
    use windows::core::PWSTR;

    fn class_name(hwnd: HWND) -> String {
        let mut b = [0u16; 128];
        // SAFETY: valid buffer.
        let n = unsafe { GetClassNameW(hwnd, &mut b) };
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }

    /// DWM extended frame bounds (the visible frame without the invisible resize borders).
    fn visible_bounds(hwnd: HWND) -> Option<RECT> {
        let mut r = RECT::default();
        // SAFETY: `r` is a RECT of the exact size passed.
        unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&raw mut r).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
        }
        .ok()
        .map(|()| r)
    }

    fn covers_monitor(hwnd: HWND, r: RECT) -> bool {
        // SAFETY: plain Win32 queries with valid out-parameters.
        unsafe {
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
                return false;
            }
            let m = mi.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
    }

    pub fn detect() -> Option<Foreground> {
        // SAFETY: Win32 queries with valid out-parameters; handles are closed.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return None;
            }
            // The desktop itself.
            if matches!(
                class_name(hwnd).as_str(),
                "Progman" | "WorkerW" | "Shell_TrayWnd"
            ) {
                return None;
            }
            let mut title = [0u16; 256];
            let tl = GetWindowTextW(hwnd, &mut title);
            let window_title = (tl > 0)
                .then(|| {
                    String::from_utf16_lossy(&title[..tl as usize])
                        .trim()
                        .to_string()
                })
                .filter(|s| !s.is_empty());

            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut exe_path = None;
            if pid != 0
                && let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            {
                let mut buf = [0u16; 1024];
                let mut size = buf.len() as u32;
                if QueryFullProcessImageNameW(
                    h,
                    PROCESS_NAME_WIN32,
                    PWSTR(buf.as_mut_ptr()),
                    &mut size,
                )
                .is_ok()
                {
                    exe_path = Some(String::from_utf16_lossy(&buf[..size as usize]));
                }
                let _ = CloseHandle(h);
            }
            let process_name = exe_path
                .as_deref()
                .and_then(|p| Path::new(p).file_name())
                .and_then(|s| s.to_str())
                .map(str::to_string)?;

            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let has_caption = style & WS_CAPTION.0 == WS_CAPTION.0;
            let shell_state = SHQueryUserNotificationState().ok();
            let fullscreen = if shell_state == Some(QUNS_RUNNING_D3D_FULL_SCREEN) {
                Fullscreen::Exclusive
            } else if !has_caption && visible_bounds(hwnd).is_some_and(|r| covers_monitor(hwnd, r))
            {
                Fullscreen::Borderless
            } else if shell_state == Some(QUNS_BUSY)
                && visible_bounds(hwnd).is_some_and(|r| covers_monitor(hwnd, r))
            {
                // The shell flags a full-monitor window as a fullscreen app even with a caption.
                Fullscreen::Borderless
            } else {
                Fullscreen::No
            };
            Some(Foreground {
                process_name,
                exe_path,
                window_title,
                fullscreen,
            })
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Foreground;
    use super::Fullscreen;
    use crate::session::exec::run_ok;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(2);

    pub fn is_x11() -> bool {
        let wayland = std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
            || std::env::var_os("WAYLAND_DISPLAY").is_some();
        !wayland && std::env::var_os("DISPLAY").is_some()
    }

    /// `_NET_WM_PID(CARDINAL) = 1234`
    pub(crate) fn parse_xprop_pid(text: &str) -> Option<u32> {
        text.lines()
            .find(|l| l.starts_with("_NET_WM_PID"))?
            .split('=')
            .nth(1)?
            .trim()
            .parse()
            .ok()
    }

    /// `_NET_WM_NAME(UTF8_STRING) = "title"`
    pub(crate) fn parse_xprop_title(text: &str) -> Option<String> {
        let l = text.lines().find(|l| l.starts_with("_NET_WM_NAME"))?;
        let v = l.split_once('=')?.1.trim();
        let v = v.strip_prefix('"')?.strip_suffix('"')?;
        (!v.is_empty()).then(|| v.to_string())
    }

    pub(crate) fn xprop_is_fullscreen(text: &str) -> bool {
        text.lines()
            .any(|l| l.starts_with("_NET_WM_STATE") && l.contains("_NET_WM_STATE_FULLSCREEN"))
    }

    pub fn detect() -> Option<Foreground> {
        if !is_x11() {
            return None;
        }
        let id = run_ok("xdotool", &["getactivewindow"], T).ok()?;
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let props = run_ok(
            "xprop",
            &["-id", id, "_NET_WM_PID", "_NET_WM_NAME", "_NET_WM_STATE"],
            T,
        )
        .ok()?;
        let pid = parse_xprop_pid(&props)?;
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()?
            .trim()
            .to_string();
        let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        // Wine / Proton games are `wine64-preloader`; the game folder shows up in the command line.
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "));
        let hint_path = match (&exe, &cmdline) {
            (Some(e), Some(c)) => Some(format!("{e} {c}")),
            (Some(e), None) => Some(e.clone()),
            (None, Some(c)) => Some(c.clone()),
            _ => None,
        };
        Some(Foreground {
            process_name: comm,
            exe_path: hint_path,
            window_title: parse_xprop_title(&props),
            fullscreen: if xprop_is_fullscreen(&props) {
                Fullscreen::Borderless
            } else {
                Fullscreen::No
            },
        })
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::Foreground;
    pub fn detect() -> Option<Foreground> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(proc_: &str, path: Option<&str>, fs: Fullscreen, gpu: Option<u32>) -> bool {
        is_gaming(GamingInput {
            process_name: proc_,
            exe_path: path,
            fullscreen: fs,
            gpu_load_pct: gpu,
        })
    }

    #[test]
    fn a_maximised_window_is_not_a_game() {
        // The old geometry test flagged these.
        assert!(!g("notepad.exe", None, Fullscreen::No, Some(99)));
        assert!(!g("game.exe", None, Fullscreen::No, Some(99)));
    }

    #[test]
    fn exclusive_fullscreen_is_gaming_unless_denied() {
        assert!(g("eldenring.exe", None, Fullscreen::Exclusive, None));
        assert!(!g("chrome.exe", None, Fullscreen::Exclusive, Some(90)));
        assert!(!g("vlc.exe", None, Fullscreen::Exclusive, Some(90)));
        assert!(!g("explorer.exe", None, Fullscreen::Exclusive, None));
    }

    #[test]
    fn borderless_needs_gpu_load_or_a_game_folder() {
        assert!(g("game.exe", None, Fullscreen::Borderless, Some(75)));
        assert!(!g("game.exe", None, Fullscreen::Borderless, Some(10)));
        assert!(
            !g("game.exe", None, Fullscreen::Borderless, None),
            "unknown load is not assumed high"
        );
        assert!(g(
            "game.exe",
            Some(r"D:\SteamLibrary\steamapps\common\Some Game\game.exe"),
            Fullscreen::Borderless,
            Some(5)
        ));
        // A game folder does not make a *windowed* game "gaming mode".
        assert!(!g(
            "game.exe",
            Some(r"C:\Program Files (x86)\Steam\steamapps\common\X\x.exe"),
            Fullscreen::No,
            Some(90)
        ));
        // Deny list wins over path hints.
        assert!(!g(
            "chrome.exe",
            Some("/steamapps/common/x"),
            Fullscreen::Borderless,
            Some(90)
        ));
    }

    #[test]
    fn empty_process_is_never_gaming() {
        assert!(!g("", None, Fullscreen::Exclusive, Some(100)));
    }

    #[test]
    fn gpu_probe_does_not_panic_without_nvml() {
        let mut p = GpuLoadProbe::new();
        let _ = p.load_pct();
        let _ = p.load_pct();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn xprop_parsing() {
        let t = "_NET_WM_PID(CARDINAL) = 4242\n_NET_WM_NAME(UTF8_STRING) = \"Some Game\"\n_NET_WM_STATE(ATOM) = _NET_WM_STATE_FULLSCREEN, _NET_WM_STATE_FOCUSED\n";
        assert_eq!(imp::parse_xprop_pid(t), Some(4242));
        assert_eq!(imp::parse_xprop_title(t).as_deref(), Some("Some Game"));
        assert!(imp::xprop_is_fullscreen(t));
    }
}
