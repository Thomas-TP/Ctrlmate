//! App self-update with Velopack (per-user install, no admin). Channels follow the packaging
//! notes (`win`, `linux`, `linux-arm64`). Nothing is downloaded or applied unless the UI asked:
//! `install` additionally requires that the last explicit `check` found an update.

use crate::error::{ErrorCode, IpcError};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, UpdateOptions, sources};

/// GitHub repository that hosts the Velopack releases (configurable here).
pub const GITHUB_REPO_URL: &str = "https://github.com/Thomas-TP/Ctrlmate";

pub const EVENT_PROGRESS: &str = "app-update://progress";

pub const fn channel() -> &'static str {
    if cfg!(target_os = "windows") {
        "win"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-arm64"
    } else {
        "linux"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    /// Not installed through Velopack (portable run, distro package, dev build).
    Unsupported,
    UpToDate,
    Available,
    /// The feed could not be read (offline, rate limited...).
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppUpdateStatus {
    pub state: UpdateState,
    pub channel: &'static str,
    pub current_version: String,
    pub latest_version: Option<String>,
    /// Technical detail of an error (never shown as a sentence).
    pub detail: Option<String>,
}

#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<UpdateInfo>>);

fn manager() -> Result<UpdateManager, velopack::Error> {
    let source = sources::GithubSource::new(GITHUB_REPO_URL, None, false);
    let options = UpdateOptions {
        ExplicitChannel: Some(channel().to_string()),
        ..UpdateOptions::default()
    };
    UpdateManager::new(source, Some(options), None)
}

fn status(state: UpdateState, latest: Option<String>, detail: Option<String>) -> AppUpdateStatus {
    AppUpdateStatus {
        state,
        channel: channel(),
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        latest_version: latest,
        detail,
    }
}

pub async fn check(pending: &PendingUpdate) -> Result<AppUpdateStatus, IpcError> {
    let result = tauri::async_runtime::spawn_blocking(|| {
        let um = match manager() {
            Ok(um) => um,
            Err(e) => {
                return (
                    status(UpdateState::Unsupported, None, Some(e.to_string())),
                    None,
                );
            }
        };
        match um.check_for_updates() {
            Ok(UpdateCheck::UpdateAvailable(info)) => {
                let v = info.TargetFullRelease.Version.clone();
                (status(UpdateState::Available, Some(v), None), Some(*info))
            }
            Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => {
                (status(UpdateState::UpToDate, None, None), None)
            }
            Err(e) => (status(UpdateState::Error, None, Some(e.to_string())), None),
        }
    })
    .await
    .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))?;
    if let Ok(mut g) = pending.0.lock() {
        *g = result.1;
    }
    Ok(result.0)
}

/// Downloads and applies the update found by the last `check`, then restarts the app.
pub async fn install(app: AppHandle, pending: &PendingUpdate) -> Result<(), IpcError> {
    let info = pending
        .0
        .lock()
        .ok()
        .and_then(|mut g| g.take())
        .ok_or(IpcError::new(ErrorCode::NothingToDo))?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), IpcError> {
        let um =
            manager().map_err(|e| IpcError::with_detail(ErrorCode::NotSupported, e.to_string()))?;
        let (tx, rx) = std::sync::mpsc::channel::<i16>();
        let progress_app = app.clone();
        std::thread::spawn(move || {
            for pct in rx {
                let _ = progress_app.emit(EVENT_PROGRESS, pct);
            }
        });
        um.download_updates(&info, Some(tx))
            .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))?;
        um.apply_updates_and_restart(&info)
            .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))
    })
    .await
    .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_matches_packaging_notes() {
        assert!(["win", "linux", "linux-arm64"].contains(&channel()));
    }
}
