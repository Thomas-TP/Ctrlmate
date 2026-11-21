//! Real OS Update Checker & Installer (Windows Update Agent COM API / Linux PackageKit & fwupd).

use crate::system::os_version_string;
use anyhow::Result;
use chrono::Utc;
use ctrlmate_protocol::OsUpdateState;
use tracing::{info, warn};

/// Performs a blocking OS update scan (run on a dedicated blocking thread, never the Tokio executor).
pub fn check_os_updates() -> OsUpdateState {
    let installed = os_version_string();
    match imp::scan_updates(&installed) {
        Ok(st) => st,
        Err(err) => {
            warn!("OS update scan failed: {err}");
            OsUpdateState {
                installed_version: installed.clone(),
                latest_version: installed,
                pending_count: 0,
                pending_titles: Vec::new(),
                in_progress: false,
                reboot_required: imp::is_reboot_required(),
                last_checked: Some(Utc::now()),
            }
        }
    }
}

/// Checks whether the OS currently flags a pending reboot (lightweight, fast).
pub fn is_reboot_required() -> bool {
    imp::is_reboot_required()
}

/// Downloads and installs pending OS updates (blocking; run on a background thread).
pub fn install_os_updates() -> Result<OsUpdateState> {
    let installed = os_version_string();
    info!("Starting OS update download & installation...");
    imp::install_updates(&installed)
}

#[cfg(target_os = "windows")]
mod imp {
    use anyhow::{bail, Context, Result};
    use chrono::Utc;
    use ctrlmate_protocol::OsUpdateState;
    use tracing::info;
    use windows::core::BSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::UpdateAgent::{
        IUpdateCollection, IUpdateSession, UpdateCollection, UpdateSession,
    };
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    struct ComScope(bool);
    impl ComScope {
        fn enter() -> Self {
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            Self(hr.is_ok())
        }
    }
    impl Drop for ComScope {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    pub fn is_reboot_required() -> bool {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let cbs = hklm
            .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending")
            .is_ok();
        let wu = hklm
            .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired")
            .is_ok();
        cbs || wu
    }

    pub fn scan_updates(installed_version: &str) -> Result<OsUpdateState> {
        let _com = ComScope::enter();
        unsafe {
            let session: IUpdateSession =
                CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)
                    .context("Failed to create Windows Update Session")?;
            let searcher = session.CreateUpdateSearcher()?;
            let criteria = BSTR::from("IsInstalled=0 and IsHidden=0 and Type='Software'");
            let result = searcher.Search(&criteria)?;
            let updates = result.Updates()?;
            let count = updates.Count()?;

            let mut titles = Vec::with_capacity(count.max(0) as usize);
            for i in 0..count {
                if let Ok(upd) = updates.get_Item(i) {
                    if let Ok(title) = upd.Title() {
                        titles.push(title.to_string());
                    }
                }
            }

            let pending_count = titles.len() as u32;
            let latest_version = if pending_count > 0 {
                format!("{installed_version} (+{pending_count})")
            } else {
                installed_version.to_string()
            };

            info!("Windows Update scan complete: {pending_count} pending update(s)");
            Ok(OsUpdateState {
                installed_version: installed_version.to_string(),
                latest_version,
                pending_count,
                pending_titles: titles,
                in_progress: false,
                reboot_required: is_reboot_required(),
                last_checked: Some(Utc::now()),
            })
        }
    }

    pub fn install_updates(installed_version: &str) -> Result<OsUpdateState> {
        let _com = ComScope::enter();
        unsafe {
            let session: IUpdateSession =
                CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
            let searcher = session.CreateUpdateSearcher()?;
            let criteria = BSTR::from("IsInstalled=0 and IsHidden=0 and Type='Software'");
            let result = searcher.Search(&criteria)?;
            let found = result.Updates()?;
            let count = found.Count()?;

            if count <= 0 {
                return Ok(OsUpdateState {
                    installed_version: installed_version.to_string(),
                    latest_version: installed_version.to_string(),
                    pending_count: 0,
                    pending_titles: Vec::new(),
                    in_progress: false,
                    reboot_required: is_reboot_required(),
                    last_checked: Some(Utc::now()),
                });
            }

            let to_download: IUpdateCollection =
                CoCreateInstance(&UpdateCollection, None, CLSCTX_INPROC_SERVER)?;
            for i in 0..count {
                if let Ok(upd) = found.get_Item(i) {
                    let _ = to_download.Add(&upd);
                }
            }

            let downloader = session.CreateUpdateDownloader()?;
            downloader.SetUpdates(&to_download)?;
            let dl_res = downloader.Download()?;
            info!("Windows Update download result code: {:?}", dl_res.ResultCode());

            let to_install: IUpdateCollection =
                CoCreateInstance(&UpdateCollection, None, CLSCTX_INPROC_SERVER)?;
            let mut ready = 0;
            for i in 0..count {
                if let Ok(upd) = found.get_Item(i) {
                    if upd.IsDownloaded().map(|b| b.as_bool()).unwrap_or(false) {
                        let _ = to_install.Add(&upd);
                        ready += 1;
                    }
                }
            }

            if ready == 0 {
                bail!("No downloaded updates ready to install (Administrator privileges may be required)");
            }

            let installer = session.CreateUpdateInstaller()?;
            installer.SetUpdates(&to_install)?;
            let inst_res = installer.Install()?;
            let reboot = inst_res
                .RebootRequired()
                .map(|b| b.as_bool())
                .unwrap_or_else(|_| is_reboot_required());
            info!(
                "Windows Update installation finished: code={:?}, reboot_required={reboot}",
                inst_res.ResultCode()
            );

            // Re-scan to report accurate remaining updates
            let mut after = scan_updates(installed_version)?;
            after.reboot_required = after.reboot_required || reboot;
            Ok(after)
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::{bail, Result};
    use chrono::Utc;
    use ctrlmate_protocol::OsUpdateState;
    use std::path::Path;
    use std::process::Command;

    pub fn is_reboot_required() -> bool {
        Path::new("/var/run/reboot-required").exists()
    }

    pub fn scan_updates(installed_version: &str) -> Result<OsUpdateState> {
        let mut titles = Vec::new();
        if let Ok(out) = Command::new("pkcon").args(["get-updates", "-p"]).output() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let t = line.trim();
                if t.starts_with("Available") || t.starts_with("Security") || t.starts_with("Bug fix") {
                    titles.push(t.to_string());
                }
            }
        }
        let pending_count = titles.len() as u32;
        Ok(OsUpdateState {
            installed_version: installed_version.to_string(),
            latest_version: if pending_count > 0 {
                format!("{installed_version} (+{pending_count})")
            } else {
                installed_version.to_string()
            },
            pending_count,
            pending_titles: titles,
            in_progress: false,
            reboot_required: is_reboot_required(),
            last_checked: Some(Utc::now()),
        })
    }

    pub fn install_updates(installed_version: &str) -> Result<OsUpdateState> {
        let status = Command::new("pkcon").args(["update", "-y"]).status()?;
        if !status.success() {
            bail!("pkcon update failed (root/polkit privileges required)");
        }
        scan_updates(installed_version)
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use anyhow::Result;
    use chrono::Utc;
    use ctrlmate_protocol::OsUpdateState;

    pub fn is_reboot_required() -> bool {
        false
    }
    pub fn scan_updates(installed_version: &str) -> Result<OsUpdateState> {
        Ok(OsUpdateState {
            installed_version: installed_version.to_string(),
            latest_version: installed_version.to_string(),
            pending_count: 0,
            pending_titles: Vec::new(),
            in_progress: false,
            reboot_required: false,
            last_checked: Some(Utc::now()),
        })
    }
    pub fn install_updates(installed_version: &str) -> Result<OsUpdateState> {
        scan_updates(installed_version)
    }
}
