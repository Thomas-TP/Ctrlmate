//! Active foreground application & fullscreen gaming mode detection (Windows & Linux).

#[derive(Debug, Clone, Default)]
pub struct ActiveAppInfo {
    pub app_name: String,
    pub process_name: String,
    pub window_title: String,
    pub is_fullscreen: bool,
    pub gaming_mode: bool,
}

const SHELL_PROCESSES: &[&str] = &[
    "explorer.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "lockapp.exe",
    "applicationframehost.exe",
    "dwm.exe",
    "ctrlmate-desktop.exe",
];

const NON_GAME_FULLSCREEN: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "code.exe",
    "vlc.exe",
    "mpv.exe",
    "mstsc.exe",
    "windowsterminal.exe",
    "powerpnt.exe",
];

pub fn poll_active_app(gpu_load_pct: Option<u32>) -> ActiveAppInfo {
    let mut info = imp::detect_foreground();
    let lower_proc = info.process_name.to_ascii_lowercase();
    let is_shell = SHELL_PROCESSES.iter().any(|s| *s == lower_proc);
    let is_browser_or_media = NON_GAME_FULLSCREEN.iter().any(|s| *s == lower_proc);

    // Gaming mode: fullscreen non-shell app (and if it's a browser/player, only if GPU load is high)
    info.gaming_mode = !info.process_name.is_empty()
        && !is_shell
        && info.is_fullscreen
        && (!is_browser_or_media || gpu_load_pct.unwrap_or(0) >= 40);
    info
}

#[cfg(target_os = "windows")]
mod imp {
    use super::ActiveAppInfo;
    use std::path::Path;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    };

    pub fn detect_foreground() -> ActiveAppInfo {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return ActiveAppInfo {
                    app_name: "Desktop".into(),
                    ..Default::default()
                };
            }

            let mut title_buf = [0u16; 256];
            let title_len = GetWindowTextW(hwnd, &mut title_buf);
            let window_title = if title_len > 0 {
                String::from_utf16_lossy(&title_buf[..title_len as usize])
                    .trim()
                    .to_string()
            } else {
                String::new()
            };

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));

            let mut process_name = String::new();
            if pid != 0 {
                if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let mut path_buf = [0u16; 512];
                    let mut size = path_buf.len() as u32;
                    if QueryFullProcessImageNameW(
                        handle,
                        PROCESS_NAME_WIN32,
                        PWSTR(path_buf.as_mut_ptr()),
                        &mut size,
                    )
                    .is_ok()
                    {
                        let full = String::from_utf16_lossy(&path_buf[..size as usize]);
                        if let Some(fname) = Path::new(&full).file_name().and_then(|s| s.to_str()) {
                            process_name = fname.to_string();
                        }
                    }
                    let _ = CloseHandle(handle);
                }
            }

            // Fullscreen detection against the window's monitor bounds
            let mut is_fullscreen = false;
            let mut win_rect = RECT::default();
            if GetWindowRect(hwnd, &mut win_rect).is_ok() {
                let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                    let mon = mi.rcMonitor;
                    is_fullscreen = win_rect.left <= mon.left + 2
                        && win_rect.top <= mon.top + 2
                        && win_rect.right >= mon.right - 2
                        && win_rect.bottom >= mon.bottom - 2;
                }
            }

            let app_name = if !process_name.is_empty() {
                process_name
                    .strip_suffix(".exe")
                    .or_else(|| process_name.strip_suffix(".EXE"))
                    .unwrap_or(&process_name)
                    .to_string()
            } else if !window_title.is_empty() {
                window_title.clone()
            } else {
                "Desktop".into()
            };

            ActiveAppInfo {
                app_name,
                process_name,
                window_title,
                is_fullscreen,
                gaming_mode: false,
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::ActiveAppInfo;
    use std::process::Command;

    pub fn detect_foreground() -> ActiveAppInfo {
        if let Ok(out) = Command::new("xdotool")
            .args(["getactivewindow", "getwindowname"])
            .output()
        {
            if out.status.success() {
                let title = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !title.is_empty() {
                    return ActiveAppInfo {
                        app_name: title.clone(),
                        process_name: title.clone(),
                        window_title: title,
                        is_fullscreen: false,
                        gaming_mode: false,
                    };
                }
            }
        }
        ActiveAppInfo {
            app_name: "Desktop".into(),
            ..Default::default()
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::ActiveAppInfo;
    pub fn detect_foreground() -> ActiveAppInfo {
        ActiveAppInfo::default()
    }
}
