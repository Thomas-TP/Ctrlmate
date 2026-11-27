//! Ctrlmate desktop (Tauri v2): UI only. The webview never talks to `ctrlmated` and never sees the
//! IPC token: it calls the commands below, and receives daemon events through Tauri events.
//!
//! Commands: `app_info`, `ipc_request`, `connection_status`, `daemon_reconnect`, `service_status`,
//! `service_install_or_upgrade`, `agent_install_autostart`, `app_update_check`,
//! `app_update_install`, `app_settings_get`, `app_settings_set`, `set_tray_labels`,
//! `set_window_theme`, `window_hide`, `open_main`, `tray_hide`, `app_quit`.
//! Events: `daemon://event`, `daemon://connection`, `app-update://progress`, `ui://open`.
//!
//! Two windows share one frontend bundle: `main` (the app) and `tray` (the quick panel opened by
//! the notification-area icon; hidden when it loses focus).

mod autostart;
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent, Wry,
};
use updater::{AppUpdateStatus, PendingUpdate};
use ws::{ConnectionHub, ConnectionStatus};

const MAIN_WINDOW: &str = "main";
const TRAY_WINDOW: &str = "tray";
const TRAY_ID: &str = "main-tray";
const MENU_PANEL: &str = "panel";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";
/// The tray panel asks the main window to open a section (payload: the section id).
const EVENT_OPEN_SHEET: &str = "ui://open";
/// Logical size of the quick panel.
const PANEL_SIZE: (f64, f64) = (360.0, 600.0);
/// Clicking the tray icon while the panel is open first blurs (and hides) it: ignore the click that
/// would immediately reopen it.
const REOPEN_GUARD_MS: u64 = 300;

static PANEL_HIDDEN_AT_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
const ENV_PORT: &str = "CTRLMATE_IPC_PORT";

struct TrayItems {
    panel: MenuItem<Wry>,
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
        // No daemon answers, or none ever created its token: the service is not installed or never
        // ran. That is exactly when the install button must be offered, not an error.
        Err(e) if matches!(e.code, ErrorCode::DaemonOffline | ErrorCode::TokenNotFound) => {
            Ok(service::build_status(version, None))
        }
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

/// Registers (or removes) the OS launch-at-login entry. Enabling it also sets up the session agent
/// (its own autostart entry, started right away) because lock, audio, notifications and scripts
/// depend on it. Debug builds never touch the OS entries: they would point at a dev binary.
fn apply_launch_at_login(on: bool) -> std::io::Result<()> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    autostart::set_enabled(on)?;
    if on {
        tauri::async_runtime::spawn(async {
            let _ = service::install_agent_autostart().await;
        });
        service::start_agent_detached();
    }
    Ok(())
}

#[tauri::command]
fn app_settings_set(state: State<'_, AppState>, settings: AppSettings) -> Result<(), IpcError> {
    apply_launch_at_login(settings.launch_at_login)
        .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))?;
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
fn set_tray_labels(state: State<'_, AppState>, panel: String, show: String, quit: String) {
    if let Ok(g) = state.tray.lock()
        && let Some(items) = g.as_ref()
    {
        let _ = items.panel.set_text(panel);
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

/// Shows the main window (closing the tray panel), optionally opening a section of the app.
#[tauri::command]
fn open_main(app: AppHandle, sheet: Option<String>) {
    hide_panel(&app);
    show_main(&app);
    if let Some(sheet) = sheet {
        let _ = app.emit_to(MAIN_WINDOW, EVENT_OPEN_SHEET, sheet);
    }
}

#[tauri::command]
fn tray_hide(app: AppHandle) {
    hide_panel(&app);
}

fn hide_panel(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(TRAY_WINDOW) {
        let _ = w.hide();
    }
}

/// Opens the quick panel near the tray icon (`anchor` = where the icon was clicked), or hides it.
fn toggle_panel(app: &AppHandle, anchor: Option<PhysicalPosition<f64>>) {
    let Some(w) = app.get_webview_window(TRAY_WINDOW) else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    if now_ms().saturating_sub(PANEL_HIDDEN_AT_MS.load(Ordering::Relaxed)) < REOPEN_GUARD_MS {
        return;
    }
    position_panel(&w, anchor);
    let _ = w.show();
    let _ = w.set_focus();
}

/// Puts the panel in the corner of the work area closest to the click (the taskbar side), kept
/// fully on screen. Without an anchor (Linux menu entry) it goes to the bottom-right.
fn position_panel(w: &WebviewWindow, anchor: Option<PhysicalPosition<f64>>) {
    let monitor = anchor
        .and_then(|p| w.app_handle().monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| w.primary_monitor().ok().flatten());
    let Some(m) = monitor else {
        return;
    };
    let area = m.work_area();
    let Ok(size) = w.outer_size() else {
        return;
    };
    let margin = (12.0 * m.scale_factor()).round() as i32;
    let (aw, ah) = (
        i32::try_from(area.size.width).unwrap_or(i32::MAX),
        i32::try_from(area.size.height).unwrap_or(i32::MAX),
    );
    let (pw, ph) = (
        i32::try_from(size.width).unwrap_or(0),
        i32::try_from(size.height).unwrap_or(0),
    );
    let right = area.position.x + aw - pw - margin;
    let left = area.position.x + margin;
    let x = match anchor {
        Some(p) => ((p.x as i32) - pw / 2).clamp(left, right.max(left)),
        None => right.max(left),
    };
    let top = area.position.y + margin;
    let bottom = area.position.y + ah - ph - margin;
    let taskbar_on_top = anchor.is_some_and(|p| (p.y as i32) < area.position.y + ah / 2);
    let y = if taskbar_on_top { top } else { bottom.max(top) };
    let _ = w.set_position(PhysicalPosition::new(x, y));
}

fn build_panel_window(app: &tauri::App) -> tauri::Result<()> {
    WebviewWindowBuilder::new(
        app,
        TRAY_WINDOW,
        WebviewUrl::App("index.html?view=tray".into()),
    )
    .title("Ctrlmate")
    .inner_size(PANEL_SIZE.0, PANEL_SIZE.1)
    .decorations(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .shadow(true)
    .build()?;
    Ok(())
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
    let panel = MenuItem::with_id(app, MENU_PANEL, "Quick panel", true, None::<&str>)?;
    let show = MenuItem::with_id(app, MENU_SHOW, "Ctrlmate", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&panel, &show, &quit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Ctrlmate")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_PANEL => toggle_panel(app, None),
            MENU_SHOW => show_main(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                toggle_panel(tray.app_handle(), Some(position));
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(TrayItems { panel, show, quit })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // Single instance must be the first plugin.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A login-time launch (`--hidden`) must not pop the window of the running instance.
            if !args.iter().any(|a| a == autostart::HIDDEN_ARG) {
                show_main(app);
            }
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
            if let Err(e) = build_panel_window(app) {
                tracing::warn!("tray panel window unavailable: {e}");
            }
            let tray_ok = match build_tray(app) {
                Ok(items) => {
                    if let Ok(mut g) = state.tray.lock() {
                        *g = Some(items);
                    }
                    true
                }
                Err(e) => {
                    tracing::warn!("tray icon unavailable: {e}");
                    false
                }
            };
            app.manage(state);

            if settings.launch_at_login
                && let Err(e) = apply_launch_at_login(true)
            {
                tracing::warn!("could not register launch at login: {e}");
            }

            // Hidden start (setting or login launch) only makes sense with a tray icon to bring the
            // window back from; without one the window is shown so the app is never invisible.
            let launched_hidden = std::env::args().any(|a| a == autostart::HIDDEN_ARG);
            if !tray_ok || !(settings.start_hidden || launched_hidden) {
                show_main(app.handle());
            }
            ws::spawn(app.handle().clone(), hub, client);
            Ok(())
        })
        .on_window_event(|window, event| {
            // The quick panel behaves like a flyout: it closes as soon as it loses focus.
            if window.label() == TRAY_WINDOW
                && let WindowEvent::Focused(false) = event
            {
                PANEL_HIDDEN_AT_MS.store(now_ms(), Ordering::Relaxed);
                let _ = window.hide();
            }
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN_WINDOW
            {
                // Hiding only works when there is a tray icon to restore the window from.
                let to_tray = window
                    .app_handle()
                    .try_state::<AppState>()
                    .and_then(|s| {
                        let wanted = s.settings.lock().ok().map(|g| g.close_to_tray)?;
                        let has_tray = s.tray.lock().ok().is_some_and(|t| t.is_some());
                        Some(wanted && has_tray)
                    })
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
            open_main,
            tray_hide,
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
