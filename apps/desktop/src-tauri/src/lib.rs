use ctrlmate_protocol::DEFAULT_IPC_PORT;
use serde::Serialize;
use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;
use tauri::Manager;
use velopack::{sources, UpdateCheck, UpdateManager};

const GITHUB_REPO_URL: &str = "https://github.com/Thomas-TP/Ctrlmate";

#[derive(Debug, Clone, Serialize)]
pub struct AppUpdateStatus {
    pub current_version: String,
    pub installed_via_velopack: bool,
    pub update_available: bool,
    pub latest_version: Option<String>,
    pub message: String,
}

#[tauri::command]
fn get_ipc_endpoint() -> String {
    format!("http://127.0.0.1:{DEFAULT_IPC_PORT}")
}

#[tauri::command]
async fn check_app_update() -> Result<AppUpdateStatus, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let source = sources::GithubSource::new(GITHUB_REPO_URL, None, false);
        let um = match UpdateManager::new(source, None, None) {
            Ok(um) => um,
            Err(_) => {
                return Ok(AppUpdateStatus {
                    current_version: current,
                    installed_via_velopack: false,
                    update_available: false,
                    latest_version: None,
                    message: "Mode portable : installez Ctrlmate via Ctrlmate-win-Setup.exe (Velopack) pour activer les mises à jour delta automatiques.".into(),
                });
            }
        };

        match um.check_for_updates() {
            Ok(UpdateCheck::UpdateAvailable(info)) => {
                let target = info.TargetFullRelease.Version.clone();
                Ok(AppUpdateStatus {
                    current_version: current,
                    installed_via_velopack: true,
                    update_available: true,
                    latest_version: Some(target.clone()),
                    message: format!("Ctrlmate v{target} est disponible (mise à jour Velopack)."),
                })
            }
            Ok(UpdateCheck::NoUpdateAvailable) | Ok(UpdateCheck::RemoteIsEmpty) => {
                Ok(AppUpdateStatus {
                    current_version: current.clone(),
                    installed_via_velopack: true,
                    update_available: false,
                    latest_version: Some(current),
                    message: "Ctrlmate est déjà à jour.".into(),
                })
            }
            Err(err) => Ok(AppUpdateStatus {
                current_version: current,
                installed_via_velopack: true,
                update_available: false,
                latest_version: None,
                message: format!("Vérification Velopack impossible : {err}"),
            }),
        }
    })
    .await
    .map_err(|e| format!("Update check task failed: {e}"))?
}

#[tauri::command]
async fn install_app_update() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let source = sources::GithubSource::new(GITHUB_REPO_URL, None, false);
        let um = UpdateManager::new(source, None, None)
            .map_err(|e| format!("Velopack non initialisé (installation requise) : {e}"))?;

        match um.check_for_updates().map_err(|e| e.to_string())? {
            UpdateCheck::UpdateAvailable(info) => {
                um.download_updates(&info, None)
                    .map_err(|e| format!("Téléchargement échoué : {e}"))?;
                um.apply_updates_and_restart(&*info)
                    .map_err(|e| format!("Application de la mise à jour échouée : {e}"))?;
                Ok("Mise à jour appliquée — redémarrage en cours…".into())
            }
            _ => Ok("Aucune mise à jour disponible.".into()),
        }
    })
    .await
    .map_err(|e| format!("Update install task failed: {e}"))?
}

/// Ensures `ctrlmated` is running locally if not already managed as a Windows/systemd service.
fn ensure_local_daemon_running() {
    let addr = SocketAddr::from(([127, 0, 0, 1], DEFAULT_IPC_PORT));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(180)).is_ok() {
        return;
    }

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };

    #[cfg(target_os = "windows")]
    let daemon_path = dir.join("ctrlmated.exe");
    #[cfg(not(target_os = "windows"))]
    let daemon_path = dir.join("ctrlmated");

    if !daemon_path.exists() {
        return;
    }

    let mut cmd = Command::new(daemon_path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    let _ = cmd.spawn();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    ensure_local_daemon_running();

    tauri::Builder::default()
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_ipc_endpoint,
            check_app_update,
            install_app_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running Ctrlmate desktop application");
}
