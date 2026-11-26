//! Autostart at login: `HKCU\...\Run` on Windows, an XDG autostart `.desktop` file on Linux.
//! Both are per-user and need no elevation.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "Ctrlmate Agent";

/// Windows `Run` value: the quoted path, nothing else.
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Content of the XDG autostart entry. `Exec` follows the Desktop Entry quoting rules: the program
/// is double-quoted and `"`, `` ` ``, `$` and `\` are backslash-escaped (and `%` is doubled).
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
        "[Desktop Entry]\nType=Application\nName={APP_NAME}\nComment=Ctrlmate session helper\nExec={q}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    )
}

/// `<config home>/autostart/ctrlmate-agent.desktop`.
pub fn desktop_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join("ctrlmate-agent.desktop")
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn xdg_config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf()))
}

pub fn write_desktop_file(config_home: &Path, exe: &Path) -> Result<PathBuf> {
    let p = desktop_path(config_home);
    std::fs::create_dir_all(p.parent().context("no parent directory")?)?;
    std::fs::write(&p, desktop_entry(exe)).with_context(|| format!("writing {}", p.display()))?;
    Ok(p)
}

pub fn remove_desktop_file(config_home: &Path) -> Result<bool> {
    let p = desktop_path(config_home);
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("removing {}", p.display())),
    }
}

#[cfg(target_os = "windows")]
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(target_os = "windows")]
pub fn set_run_value(subkey: &str, name: &str, exe: &Path) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey_with_flags(subkey, KEY_SET_VALUE)
        .context("cannot open the Run key")?;
    key.set_value(name, &run_value(exe))
        .context("cannot write the Run value")
}

#[cfg(target_os = "windows")]
pub fn remove_run_value(subkey: &str, name: &str) -> Result<bool> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(subkey, KEY_SET_VALUE)
    else {
        return Ok(false);
    };
    match key.delete_value(name) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).context("cannot delete the Run value"),
    }
}

/// Installs autostart for the running executable; returns a description of what was written.
pub fn install() -> Result<String> {
    let exe = std::env::current_exe().context("cannot resolve the executable path")?;
    #[cfg(target_os = "windows")]
    {
        set_run_value(RUN_KEY, APP_NAME, &exe)?;
        Ok(format!("HKCU\\{RUN_KEY}\\{APP_NAME} = {}", run_value(&exe)))
    }
    #[cfg(target_os = "linux")]
    {
        let home = xdg_config_home().context("cannot resolve the XDG config directory")?;
        Ok(write_desktop_file(&home, &exe)?.display().to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = exe;
        anyhow::bail!("autostart is not supported on this OS")
    }
}

/// Removes autostart; returns whether something was removed.
pub fn uninstall() -> Result<bool> {
    #[cfg(target_os = "windows")]
    {
        remove_run_value(RUN_KEY, APP_NAME)
    }
    #[cfg(target_os = "linux")]
    {
        let home = xdg_config_home().context("cannot resolve the XDG config directory")?;
        remove_desktop_file(&home)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("autostart is not supported on this OS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_value_is_quoted() {
        assert_eq!(
            run_value(Path::new(r"C:\Program Files\Ctrlmate\ctrlmate-agent.exe")),
            "\"C:\\Program Files\\Ctrlmate\\ctrlmate-agent.exe\""
        );
    }

    #[test]
    fn desktop_entry_quotes_and_escapes_exec() {
        let e = desktop_entry(Path::new("/opt/Ctrl mate/$bin/agent"));
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("Exec=\"/opt/Ctrl mate/\\$bin/agent\"\n"), "{e}");
        assert!(e.contains("Type=Application") && e.contains("Terminal=false"));
        assert!(e.contains("NoDisplay=true"));
    }

    #[test]
    fn desktop_file_install_and_uninstall() {
        let d = tempfile::tempdir().unwrap();
        let exe = Path::new("/usr/bin/ctrlmate-agent");
        let p = write_desktop_file(d.path(), exe).unwrap();
        assert_eq!(p, d.path().join("autostart/ctrlmate-agent.desktop"));
        assert!(
            std::fs::read_to_string(&p)
                .unwrap()
                .contains("Exec=\"/usr/bin/ctrlmate-agent\"")
        );
        assert!(remove_desktop_file(d.path()).unwrap());
        assert!(
            !remove_desktop_file(d.path()).unwrap(),
            "second removal is a no-op"
        );
    }

    #[cfg(windows)]
    #[test]
    fn registry_run_value_roundtrip_in_a_private_key() {
        // A throwaway key, never the real Run key.
        let key = format!(r"Software\Ctrlmate-Test-{}", std::process::id());
        let exe = Path::new(r"C:\Test Dir\agent.exe");
        set_run_value(&key, "Ctrlmate Agent", exe).unwrap();
        {
            use winreg::RegKey;
            use winreg::enums::HKEY_CURRENT_USER;
            let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey(&key).unwrap();
            let v: String = k.get_value("Ctrlmate Agent").unwrap();
            assert_eq!(v, run_value(exe));
        }
        assert!(remove_run_value(&key, "Ctrlmate Agent").unwrap());
        assert!(!remove_run_value(&key, "Ctrlmate Agent").unwrap());
        let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey(&key);
    }
}
