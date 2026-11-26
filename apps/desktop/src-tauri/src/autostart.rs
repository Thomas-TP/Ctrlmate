//! Launch-at-login for the desktop app itself, per user and without elevation:
//! `HKCU\...\Run` on Windows, an XDG autostart `.desktop` file elsewhere.
//!
//! The entry always passes [`HIDDEN_ARG`] so a login start goes to the tray instead of popping a
//! window. (The session agent has its own autostart entry, see `ctrlmate-agent`.)

use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "Ctrlmate";
/// Passed by the autostart entry: start in the tray, do not show the window.
pub const HIDDEN_ARG: &str = "--hidden";

/// Windows `Run` value: the quoted path followed by the hidden flag.
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\" {HIDDEN_ARG}", exe.display())
}

/// Content of the XDG autostart entry. `Exec` follows the Desktop Entry quoting rules: the program
/// is double-quoted and `"`, `` ` ``, `$` and `\` are backslash-escaped (and `%` is doubled).
#[cfg(any(not(windows), test))]
pub fn desktop_entry(exe: &Path) -> String {
    let mut q = String::from("\"");
    for c in exe.display().to_string().chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                q.push('\\');
                q.push(c);
            }
            '%' => q.push_str("%%"),
            _ => q.push(c),
        }
    }
    q.push('"');
    format!(
        "[Desktop Entry]\nType=Application\nName={APP_NAME}\nComment=Ctrlmate desktop (tray)\nExec={q} {HIDDEN_ARG}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

/// `<config home>/autostart/ctrlmate-desktop.desktop`.
#[cfg(any(not(windows), test))]
pub fn desktop_path(config_home: &Path) -> PathBuf {
    config_home
        .join("autostart")
        .join("ctrlmate-desktop.desktop")
}

/// The binary to start at login. An AppImage must point at the image, not at its temporary mount.
fn launch_target() -> std::io::Result<PathBuf> {
    if let Some(p) = std::env::var_os("APPIMAGE").map(PathBuf::from)
        && p.is_absolute()
        && p.is_file()
    {
        return Ok(p);
    }
    std::env::current_exe()
}

#[cfg(windows)]
mod platform {
    use super::{APP_NAME, launch_target, run_value};
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    pub fn set_enabled(on: bool) -> std::io::Result<()> {
        if on {
            // Always rewritten so the entry follows the executable if the install moved.
            let exe = launch_target()?;
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)?;
            key.set_value(APP_NAME, &run_value(&exe))
        } else {
            let Ok(key) =
                RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
            else {
                return Ok(());
            };
            match key.delete_value(APP_NAME) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{desktop_entry, desktop_path, launch_target};
    use std::path::PathBuf;

    fn config_home() -> std::io::Result<PathBuf> {
        directories::BaseDirs::new()
            .map(|b| b.config_dir().to_path_buf())
            .ok_or_else(|| std::io::Error::other("cannot resolve the config directory"))
    }

    pub fn set_enabled(on: bool) -> std::io::Result<()> {
        let path = desktop_path(&config_home()?);
        if on {
            let exe = launch_target()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, desktop_entry(&exe))
        } else {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }
}

/// Turns launch-at-login on (rewriting the entry) or off. Turning off when absent is a no-op.
pub fn set_enabled(on: bool) -> std::io::Result<()> {
    platform::set_enabled(on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_value_is_quoted_and_hidden() {
        assert_eq!(
            run_value(Path::new(
                r"C:\Users\me\AppData\Local\Ctrlmate\current\ctrlmate-desktop.exe"
            )),
            "\"C:\\Users\\me\\AppData\\Local\\Ctrlmate\\current\\ctrlmate-desktop.exe\" --hidden"
        );
    }

    #[test]
    fn desktop_entry_quotes_escapes_and_hides() {
        let e = desktop_entry(Path::new("/opt/my app/$x/ctrlmate-desktop"));
        assert!(e.contains("Exec=\"/opt/my app/\\$x/ctrlmate-desktop\" --hidden\n"));
        assert!(e.contains("X-GNOME-Autostart-enabled=true"));
        assert!(
            !e.contains("NoDisplay"),
            "must stay visible to autostart managers"
        );
    }

    #[test]
    fn desktop_path_layout() {
        assert_eq!(
            desktop_path(Path::new("/home/u/.config")),
            PathBuf::from("/home/u/.config/autostart/ctrlmate-desktop.desktop")
        );
    }
}
