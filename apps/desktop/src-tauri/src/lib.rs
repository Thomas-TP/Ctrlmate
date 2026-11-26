//! Ctrlmate desktop (Tauri v2): UI only. The webview never talks to `ctrlmated` and never sees the
//! IPC token: it calls the commands below, and receives daemon events through Tauri events.
//!
//! Commands: `app_info`, `ipc_request`, `connection_status`, `daemon_reconnect`, `service_status`,
//! `service_install_or_upgrade`, `agent_install_autostart`, `app_update_check`,
//! `app_update_install`, `app_settings_get`, `app_settings_set`, `set_tray_labels`,
//! `set_window_theme`, `window_hide`, `app_quit`.
//! Events: `daemon://event`, `daemon://connection`, `app-update://progress`.

mod client;
#[cfg(debug_assertions)]
mod dev_daemon;
mod error;
mod service;
mod settings;
mod token;
mod updater;
mod ws;

use client::{IpcClient, IpcResponse};
use ctrlmate_protocol::DEFAULT_IPC_PORT;
use error::{ErrorCode, IpcError};
use serde::Serialize;
use serde_json::Value;
use service::{AutostartOutcome, ElevatedOutcome, ServiceAction, ServiceStatus};
use settings::AppSettings;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent, Wry};
use updater::{AppUpdateStatus, PendingUpdate};
use ws::{ConnectionHub, ConnectionStatus};

const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "main-tray";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";
const ENV_PORT: &str = "CTRLMATE_IPC_PORT";

struct TrayItems {
    show: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

pub struct AppState {
    client: Arc<IpcClient>,
    hub: Arc<ConnectionHub>,
    settings: Mutex<AppSettings>,
    settings_dir: PathBuf,
    pending_update: PendingUpdate,
    tray: Mutex<Option<TrayItems>>,
    #[cfg(debug_assertions)]
    dev_daemon: dev_daemon::DevDaemon,
}

#[derive(Debug, Clone, Serialize)]
struct AppInfo {
    version: &'static str,
    platform: &'static str,
    arch: &'static str,
    debug: bool,
    ipc_port: u16,
    /// Which token source worked last (`env`, `user`, `system`, `dev`); never the token itself.
    token_source: Option<token::TokenSource>,
    update_channel: &'static str,
}

fn ipc_port() -> u16 {
    std::env::var(ENV_PORT)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_IPC_PORT)
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: service::platform(),
        arch: std::env::consts::ARCH,
        debug: cfg!(debug_assertions),
        ipc_port: state.client.port(),
        token_source: state.client.last_source(),
        update_channel: updater::channel(),
    }
}

/// The only way for the webview to reach the daemon: allowlisted `/ipc/*` routes.
#[tauri::command]
async fn ipc_request(
    state: State<'_, AppState>,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<IpcResponse, IpcError> {
    state.client.request(&method, &path, body).await
}

#[tauri::command]
fn connection_status(state: State<'_, AppState>) -> ConnectionStatus {
    state.hub.status()
}

#[tauri::command]
fn daemon_reconnect(state: State<'_, AppState>) {
    state.hub.reconnect_now();
}

#[tauri::command]
async fn service_status(state: State<'_, AppState>) -> Result<ServiceStatus, IpcError> {
    let version = env!("CARGO_PKG_VERSION");
    match state.client.request("GET", "/ipc/snapshot", None).await {
        Ok(resp) if resp.status == 200 => {
            Ok(service::build_status(version, resp.body.get("service")))
        }
        Ok(_) => Err(IpcError::new(ErrorCode::DaemonProtocol)),
        Err(e) if e.code == ErrorCode::DaemonOffline => Ok(service::build_status(version, None)),
        Err(e) => Err(e),
    }
}

/// Elevated install / upgrade of the Windows service. The UI shows its own confirmation first.
#[tauri::command]
async fn service_install_or_upgrade(action: ServiceAction) -> Result<ElevatedOutcome, IpcError> {
    service::install_or_upgrade(action).await
}

#[tauri::command]
async fn agent_install_autostart() -> Result<AutostartOutcome, IpcError> {
    service::install_agent_autostart().await
}

#[tauri::command]
async fn app_update_check(state: State<'_, AppState>) -> Result<AppUpdateStatus, IpcError> {
    updater::check(&state.pending_update).await
}

/// Applies the update found by the last `app_update_check`, then restarts. The UI confirms first.
#[tauri::command]
async fn app_update_install(app: AppHandle, state: State<'_, AppState>) -> Result<(), IpcError> {
    updater::install(app, &state.pending_update).await
}

#[tauri::command]
fn app_settings_get(state: State<'_, AppState>) -> AppSettings {
    state.settings.lock().map(|g| *g).unwrap_or_default()
}

#[tauri::command]
fn app_settings_set(state: State<'_, AppState>, settings: AppSettings) -> Result<(), IpcError> {
    settings::save(&state.settings_dir, &settings)
        .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))?;
    if let Ok(mut g) = state.settings.lock() {
        *g = settings;
    }
    Ok(())
}

/// Tray menu labels are UI text, so the webview supplies them (translated) on start and whenever
/// the language changes.
#[tauri::command]
fn set_tray_labels(state: State<'_, AppState>, show: String, quit: String) {
    if let Ok(g) = state.tray.lock()
        && let Some(items) = g.as_ref()
    {
        let _ = items.show.set_text(show);
        let _ = items.quit.set_text(quit);
    }
}

/// Aligns the native title bar with the in-app theme override. `None` follows the system.
#[tauri::command]
fn set_window_theme(app: AppHandle, theme: Option<String>) {
    let theme = match theme.as_deref() {
        Some("light") => Some(tauri::Theme::Light),
        Some("dark") => Some(tauri::Theme::Dark),
        _ => None,
    };
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.set_theme(theme);
    }
}

#[tauri::command]
fn window_hide(app: AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.hide();
    }
}

#[tauri::command]
fn app_quit(app: AppHandle) {
    app.exit(0);
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<TrayItems> {
    // Provisional labels until the webview sends the translated ones (`set_tray_labels`).
    let show = MenuItem::with_id(app, MENU_SHOW, "Ctrlmate", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Ctrlmate")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_main(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(TrayItems { show, quit })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // Single instance must be the first plugin.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .setup(|app| {
            let settings_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("ctrlmate-desktop"));
            let settings = settings::load(&settings_dir);
            let client = Arc::new(IpcClient::new(ipc_port())?);
            let hub = ConnectionHub::new();

            let state = AppState {
                client: client.clone(),
                hub: hub.clone(),
                settings: Mutex::new(settings),
                settings_dir,
                pending_update: PendingUpdate::default(),
                tray: Mutex::new(None),
                #[cfg(debug_assertions)]
                dev_daemon: dev_daemon::DevDaemon::default(),
            };
            #[cfg(debug_assertions)]
            state.dev_daemon.spawn_if_enabled();
            match build_tray(app) {
                Ok(items) => {
                    if let Ok(mut g) = state.tray.lock() {
                        *g = Some(items);
                    }
                }
                Err(e) => tracing::warn!("tray icon unavailable: {e}"),
            }
            app.manage(state);

            if !settings.start_hidden {
                show_main(app.handle());
            }
            ws::spawn(app.handle().clone(), hub, client);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN_WINDOW
            {
                let to_tray = window
                    .app_handle()
                    .try_state::<AppState>()
                    .and_then(|s| s.settings.lock().ok().map(|g| g.close_to_tray))
                    .unwrap_or(true);
                if to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            ipc_request,
            connection_status,
            daemon_reconnect,
            service_status,
            service_install_or_upgrade,
            agent_install_autostart,
            app_update_check,
            app_update_install,
            app_settings_get,
            app_settings_set,
            set_tray_labels,
            set_window_theme,
            window_hide,
            app_quit,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Ctrlmate desktop application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            #[cfg(debug_assertions)]
            if let Some(state) = handle.try_state::<AppState>() {
                state.dev_daemon.kill();
            }
            #[cfg(not(debug_assertions))]
            let _ = handle;
        }
    });
}
