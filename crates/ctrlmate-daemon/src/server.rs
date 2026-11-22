//! HTTPS/WSS Home Assistant Server + Loopback Local IPC Server for the Tauri v2 Desktop App.

use crate::auth::{issue_client_token, verify_client_token, PairingManager};
use crate::config::DaemonConfig;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use ctrlmate_hal::audio;
use ctrlmate_hal::os_control::{
    execute_allowlisted_script, execute_power_action, send_desktop_notification,
};
use ctrlmate_hal::smart_poll::{ActivityTier, SmartPoller};
use ctrlmate_hal::{updates, HardwareCollector};
use ctrlmate_protocol::{
    AllowlistedScript, ControlCommand, CoolingPreset, DaemonSnapshot, DeviceManifest, EntityState,
    EntityValue, MqttConfig, PairRequest, PairResponse, PairedClientInfo, PollingProfile,
    RemotePermissions, SecurityMode, TemperatureUnit, WireMessage, PROTOCOL_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{debug, info, warn};

pub struct AppState {
    pub data_dir: PathBuf,
    pub ha_port: u16,
    pub cert_fingerprint: String,
    pub config: RwLock<DaemonConfig>,
    pub collector: Mutex<HardwareCollector>,
    pub pairing: Mutex<PairingManager>,
    pub poller: Mutex<SmartPoller>,
    pub latest_manifest: RwLock<DeviceManifest>,
    pub latest_states: RwLock<Vec<EntityState>>,
    pub connected_clients: RwLock<HashSet<String>>,
    /// Broadcasts smart-filtered batches & manifest updates to connected Home Assistant WebSockets.
    pub ha_tx: broadcast::Sender<WireMessage>,
    /// Broadcasts 1 Hz live snapshots to the local desktop UI.
    pub ui_tx: broadcast::Sender<DaemonSnapshot>,
    /// Live connection status of the optional MQTT Discovery transport.
    pub mqtt_connected: AtomicBool,
    /// Notifies the MQTT worker when configuration changes at runtime.
    pub mqtt_cfg_tx: watch::Sender<MqttConfig>,
}

impl AppState {
    pub async fn snapshot(&self) -> DaemonSnapshot {
        let manifest = self.latest_manifest.read().await.clone();
        let states = self.latest_states.read().await.clone();
        let cfg = self.config.read().await.clone();
        let connected = self.connected_clients.read().await.clone();

        let (conflicts, hid_devices, failsafe, rgb_backends, rgb_devices, deep_bridge_active) = {
            let col = self.collector.lock().await;
            (
                col.detect_conflicts(),
                col.hid_devices().to_vec(),
                col.is_thermal_failsafe_active(),
                col.rgb_backend_names(),
                col.rgb_device_statuses(),
                col.deep_bridge_active(),
            )
        };

        let pairing = {
            let mut pm = self.pairing.lock().await;
            pm.status(&self.cert_fingerprint, self.ha_port)
        };

        let paired_clients = cfg
            .paired_clients
            .iter()
            .map(|c| PairedClientInfo {
                client_id: c.client_id.clone(),
                client_name: c.client_name.clone(),
                paired_at: c.paired_at,
                connected: connected.contains(&c.client_id),
            })
            .collect();

        DaemonSnapshot {
            manifest,
            states,
            conflicts,
            hid_devices,
            pairing,
            paired_clients,
            permissions: cfg.permissions,
            polling_profile: cfg.polling_profile,
            temperature_unit: cfg.temperature_unit,
            scripts: cfg.scripts,
            thermal_failsafe_active: failsafe,
            rgb_backends,
            rgb_devices,
            deep_bridge_active,
            mqtt: cfg.mqtt,
            mqtt_connected: self.mqtt_connected.load(Ordering::Relaxed),
        }
    }

    /// Rebuilds the DeviceManifest (e.g. audio devices plugged in or allowlisted scripts edited)
    /// and pushes the updated manifest live to all connected Home Assistant instances.
    pub async fn refresh_manifest(&self) {
        let scripts = self.config.read().await.scripts.clone();
        let manifest = {
            let mut col = self.collector.lock().await;
            col.build_manifest(&scripts)
        };
        *self.latest_manifest.write().await = manifest.clone();
        let _ = self.ha_tx.send(WireMessage::Manifest(manifest));
        let _ = self.ui_tx.send(self.snapshot().await);
    }

    /// Spawns a non-blocking background OS update scan (WUA / PackageKit) and pushes results.
    pub fn spawn_update_scan(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(updates::check_os_updates).await;
            if let Ok(upd) = result {
                let now = chrono::Utc::now();
                let reboot = upd.reboot_required;
                {
                    let mut col = state.collector.lock().await;
                    col.set_os_updates(upd.clone());
                }
                state
                    .merge_and_broadcast_states(vec![
                        EntityState {
                            id: "os_updates".into(),
                            value: EntityValue::Update(upd),
                            attributes: Default::default(),
                            updated_at: now,
                        },
                        EntityState {
                            id: "reboot_required".into(),
                            value: EntityValue::Bool(reboot),
                            attributes: Default::default(),
                            updated_at: now,
                        },
                    ])
                    .await;
            }
        });
    }

    /// Spawns a non-blocking OS update download & installation worker.
    pub fn spawn_update_install(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let now = chrono::Utc::now();
            let in_prog = {
                let mut col = state.collector.lock().await;
                col.set_os_updates_in_progress(true);
                col.os_updates().clone()
            };
            state
                .merge_and_broadcast_states(vec![EntityState {
                    id: "os_updates".into(),
                    value: EntityValue::Update(in_prog),
                    attributes: Default::default(),
                    updated_at: now,
                }])
                .await;

            match tokio::task::spawn_blocking(updates::install_os_updates).await {
                Ok(Ok(final_state)) => {
                    let reboot = final_state.reboot_required;
                    {
                        let mut col = state.collector.lock().await;
                        col.set_os_updates(final_state.clone());
                    }
                    state
                        .merge_and_broadcast_states(vec![
                            EntityState {
                                id: "os_updates".into(),
                                value: EntityValue::Update(final_state),
                                attributes: Default::default(),
                                updated_at: chrono::Utc::now(),
                            },
                            EntityState {
                                id: "reboot_required".into(),
                                value: EntityValue::Bool(reboot),
                                attributes: Default::default(),
                                updated_at: chrono::Utc::now(),
                            },
                        ])
                        .await;
                }
                Ok(Err(err)) => {
                    warn!("OS update install error: {err}");
                    let reverted = {
                        let mut col = state.collector.lock().await;
                        col.set_os_updates_in_progress(false);
                        col.os_updates().clone()
                    };
                    state
                        .merge_and_broadcast_states(vec![EntityState {
                            id: "os_updates".into(),
                            value: EntityValue::Update(reverted),
                            attributes: Default::default(),
                            updated_at: chrono::Utc::now(),
                        }])
                        .await;
                }
                Err(err) => warn!("OS update install task panicked: {err}"),
            }
        });
    }

    pub async fn execute_command(
        self: &Arc<Self>,
        cmd: ControlCommand,
        from_remote: bool,
    ) -> Result<String, String> {
        let perms = self.config.read().await.permissions.clone();
        let now = chrono::Utc::now();

        match cmd {
            ControlCommand::SetLight {
                entity_id,
                is_on,
                brightness,
                rgb,
                color_temp_kelvin,
                transition_ms,
            } => {
                if from_remote && !perms.lighting {
                    return Err("Lighting control is disabled in Ctrlmate permissions.".into());
                }
                let updated = {
                    let mut col = self.collector.lock().await;
                    col.lighting_mut().apply_update(
                        &entity_id,
                        is_on,
                        brightness,
                        rgb,
                        color_temp_kelvin,
                        transition_ms,
                    )
                };
                if updated.is_empty() {
                    return Err(format!("Unknown lighting entity: {entity_id}"));
                }

                let batch: Vec<EntityState> = updated
                    .into_iter()
                    .map(|(id, st)| EntityState {
                        id,
                        value: EntityValue::Light(st),
                        attributes: Default::default(),
                        updated_at: now,
                    })
                    .collect();
                self.merge_and_broadcast_states(batch).await;
                Ok("Lighting updated".into())
            }
            ControlCommand::SetCoolingPreset {
                entity_id: _,
                preset,
            } => {
                if from_remote && !perms.cooling {
                    return Err("Cooling control is disabled in Ctrlmate permissions.".into());
                }
                {
                    let mut col = self.collector.lock().await;
                    col.cooling_mut().set_preset(preset);
                }
                self.merge_and_broadcast_states(vec![EntityState {
                    id: "cooling_preset".into(),
                    value: EntityValue::Text(preset.as_ha_option().into()),
                    attributes: Default::default(),
                    updated_at: now,
                }])
                .await;
                Ok(format!("Cooling mode set to {}", preset.as_ha_option()))
            }
            ControlCommand::SetSelect { entity_id, option } => {
                if entity_id == "cooling_preset" {
                    let preset = CoolingPreset::from_ha_option(&option)
                        .ok_or_else(|| format!("Invalid cooling preset: {option}"))?;
                    return Box::pin(self.execute_command(
                        ControlCommand::SetCoolingPreset { entity_id, preset },
                        from_remote,
                    ))
                    .await;
                }
                if entity_id == "audio_output" {
                    if from_remote && !perms.audio_and_media {
                        return Err("Audio control is disabled in Ctrlmate permissions.".into());
                    }
                    let switched = audio::set_default_output_by_name(&option)
                        .map_err(|e| e.to_string())?;
                    self.merge_and_broadcast_states(vec![EntityState {
                        id: "audio_output".into(),
                        value: EntityValue::Text(switched.clone()),
                        attributes: Default::default(),
                        updated_at: now,
                    }])
                    .await;
                    return Ok(format!("Audio output switched to {switched}"));
                }
                Err(format!("Unsupported select entity: {entity_id}"))
            }
            ControlCommand::SetNumber { entity_id, value } => {
                if entity_id == "master_volume" {
                    if from_remote && !perms.audio_and_media {
                        return Err("Audio control is disabled in Ctrlmate permissions.".into());
                    }
                    let applied = audio::set_volume(value).map_err(|e| e.to_string())?;
                    self.merge_and_broadcast_states(vec![EntityState {
                        id: "master_volume".into(),
                        value: EntityValue::Float(applied),
                        attributes: Default::default(),
                        updated_at: now,
                    }])
                    .await;
                    return Ok(format!("Master volume set to {applied:.0}%"));
                }
                Err(format!("Unsupported number entity: {entity_id}"))
            }
            ControlCommand::SetSwitch { entity_id, state } => {
                if entity_id == "audio_mute" {
                    if from_remote && !perms.audio_and_media {
                        return Err("Audio control is disabled in Ctrlmate permissions.".into());
                    }
                    let applied = audio::set_mute(state).map_err(|e| e.to_string())?;
                    self.merge_and_broadcast_states(vec![EntityState {
                        id: "audio_mute".into(),
                        value: EntityValue::Bool(applied),
                        attributes: Default::default(),
                        updated_at: now,
                    }])
                    .await;
                    return Ok(format!("Audio mute set to {applied}"));
                }
                Err(format!("Unsupported switch entity: {entity_id}"))
            }
            ControlCommand::PressButton { entity_id } => {
                if entity_id == "check_os_updates" {
                    if from_remote && !perms.os_updates {
                        return Err("OS Updates control is disabled in Ctrlmate permissions.".into());
                    }
                    self.spawn_update_scan();
                    return Ok("OS update scan started".into());
                }

                if entity_id.starts_with("power_") || entity_id == "display_off" {
                    if from_remote && !perms.power_control {
                        return Err("Power control is disabled in Ctrlmate permissions.".into());
                    }
                    return execute_power_action(&entity_id).map_err(|e| e.to_string());
                }

                if let Some(script_id) = entity_id.strip_prefix("script_") {
                    if from_remote && !perms.custom_scripts {
                        return Err("Custom scripts are disabled in Ctrlmate permissions.".into());
                    }
                    let scripts = self.config.read().await.scripts.clone();
                    let script = scripts
                        .iter()
                        .find(|s| s.id == script_id)
                        .ok_or_else(|| format!("Script not found in allowlist: {script_id}"))?;
                    return execute_allowlisted_script(script).map_err(|e| e.to_string());
                }

                Err(format!("Unknown button entity: {entity_id}"))
            }
            ControlCommand::CheckOsUpdates => {
                if from_remote && !perms.os_updates {
                    return Err("OS Updates control is disabled in Ctrlmate permissions.".into());
                }
                self.spawn_update_scan();
                Ok("OS update scan started".into())
            }
            ControlCommand::InstallOsUpdates => {
                if from_remote && !perms.os_updates {
                    return Err("OS Updates control is disabled in Ctrlmate permissions.".into());
                }
                self.spawn_update_install();
                Ok("OS update installation started in background".into())
            }
            ControlCommand::SendNotification { title, message } => {
                if from_remote && !perms.notifications {
                    return Err("Desktop notifications are disabled in Ctrlmate permissions.".into());
                }
                send_desktop_notification(&title, &message).map_err(|e| e.to_string())
            }
        }
    }

    async fn merge_and_broadcast_states(&self, batch: Vec<EntityState>) {
        {
            let mut latest = self.latest_states.write().await;
            for item in &batch {
                if let Some(existing) = latest.iter_mut().find(|s| s.id == item.id) {
                    *existing = item.clone();
                } else {
                    latest.push(item.clone());
                }
            }
        }
        let _ = self.ha_tx.send(WireMessage::StateBatch { states: batch });
        let snap = self.snapshot().await;
        let _ = self.ui_tx.send(snap);
    }

    /// Periodic 1 Hz hardware poll updating local UI and smart-filtering updates toward HA.
    pub async fn tick_poll(&self) {
        let (states, audio_outputs_changed, gaming_mode) = {
            let mut col = self.collector.lock().await;
            let (st, changed) = col.poll_states();
            (st, changed, col.is_gaming_mode())
        };

        if audio_outputs_changed {
            self.refresh_manifest().await;
        }

        {
            let mut latest = self.latest_states.write().await;
            *latest = states.clone();
        }

        // Always stream 1 Hz to open desktop UI clients.
        if self.ui_tx.receiver_count() > 0 {
            let snap = self.snapshot().await;
            let _ = self.ui_tx.send(snap);
        }

        // Smart-filter telemetry toward Home Assistant to keep Recorder lean.
        if self.ha_tx.receiver_count() > 0 {
            let tier = if gaming_mode {
                ActivityTier::Gaming
            } else {
                ActivityTier::Active
            };
            let filtered = {
                let mut poller = self.poller.lock().await;
                poller.filter_batch(&states, tier, Instant::now())
            };
            if !filtered.is_empty() {
                debug!(
                    "Pushing {} delta-filtered entity updates to HA (tier={:?})",
                    filtered.len(),
                    tier
                );
                let _ = self.ha_tx.send(WireMessage::StateBatch { states: filtered });
            }
        }
    }
}

/// Builds the LAN HTTPS/WSS Router consumed by Home Assistant 2026.11.
pub fn build_ha_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/info", get(ha_info))
        .route("/api/v1/pair", post(ha_pair))
        .route("/ws", get(ha_ws_upgrade))
        .with_state(state)
}

/// Builds the loopback-only Local IPC Router consumed by the Tauri v2 Desktop App.
pub fn build_ipc_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ipc/snapshot", get(ipc_snapshot))
        .route("/ipc/ws", get(ipc_ws_upgrade))
        .route("/ipc/pairing/start", post(ipc_start_pairing))
        .route("/ipc/pairing/cancel", post(ipc_cancel_pairing))
        .route("/ipc/clients/{client_id}", delete(ipc_revoke_client))
        .route("/ipc/command", post(ipc_command))
        .route("/ipc/settings", put(ipc_update_settings))
        .route("/ipc/hardware/rescan", post(ipc_rescan_hardware))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[derive(Serialize)]
struct DiscoveryInfoResponse {
    protocol_version: u16,
    device_id: String,
    hostname: String,
    sw_version: String,
    mac_address: Option<String>,
    cert_fingerprint: String,
    pairing_active: bool,
}

async fn ha_info(State(state): State<Arc<AppState>>) -> Json<DiscoveryInfoResponse> {
    let manifest = state.latest_manifest.read().await;
    let pairing = state
        .pairing
        .lock()
        .await
        .status(&state.cert_fingerprint, state.ha_port);

    Json(DiscoveryInfoResponse {
        protocol_version: PROTOCOL_VERSION,
        device_id: manifest.host.device_id.clone(),
        hostname: manifest.host.name.clone(),
        sw_version: manifest.host.sw_version.clone(),
        mac_address: manifest.host.mac_address.clone(),
        cert_fingerprint: state.cert_fingerprint.clone(),
        pairing_active: pairing.active,
    })
}

async fn ha_pair(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PairRequest>,
) -> impl IntoResponse {
    {
        let mut pm = state.pairing.lock().await;
        if let Err(msg) = pm.verify_and_consume(&req.pin) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": msg })),
            )
                .into_response();
        }
    }

    let (token, stored) = {
        let mut cfg = state.config.write().await;
        let (tok, st) = issue_client_token(&cfg, req.client_name.clone());
        cfg.paired_clients.push(st.clone());
        if let Err(err) = cfg.save(&state.data_dir) {
            warn!("Failed persisting paired client: {err}");
        }
        (tok, st)
    };

    let manifest = state.latest_manifest.read().await;
    info!(
        "Paired new Home Assistant client '{}' ({})",
        stored.client_name, stored.client_id
    );

    let resp = PairResponse {
        client_id: stored.client_id,
        token,
        device_id: manifest.host.device_id.clone(),
        hostname: manifest.host.name.clone(),
        mac_address: manifest.host.mac_address.clone(),
        cert_fingerprint: state.cert_fingerprint.clone(),
    };

    let snap = state.snapshot().await;
    let _ = state.ui_tx.send(snap);

    (StatusCode::OK, Json(resp)).into_response()
}

async fn ha_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ha_ws(socket, state))
}

async fn handle_ha_ws(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();

    let hello = {
        let manifest = state.latest_manifest.read().await;
        WireMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: manifest.host.device_id.clone(),
            hostname: manifest.host.name.clone(),
            sw_version: manifest.host.sw_version.clone(),
            cert_fingerprint: state.cert_fingerprint.clone(),
        }
    };
    if send_wire(&mut sink, &hello).await.is_err() {
        return;
    }

    let auth_frame = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next()).await;
    let client_id = match auth_frame {
        Ok(Some(Ok(Message::Text(txt)))) => match serde_json::from_str::<WireMessage>(&txt) {
            Ok(WireMessage::Auth { token, client_name }) => {
                let cfg = state.config.read().await;
                if let Some(cid) = verify_client_token(&cfg, &token) {
                    info!("Home Assistant client '{client_name}' ({cid}) authenticated over WSS");
                    cid
                } else {
                    let _ = send_wire(
                        &mut sink,
                        &WireMessage::AuthError {
                            reason: "Invalid or revoked bearer token".into(),
                        },
                    )
                    .await;
                    return;
                }
            }
            _ => {
                let _ = send_wire(
                    &mut sink,
                    &WireMessage::AuthError {
                        reason: "Expected Auth message".into(),
                    },
                )
                .await;
                return;
            }
        },
        _ => return,
    };

    if send_wire(
        &mut sink,
        &WireMessage::AuthOk {
            client_id: client_id.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }

    state.connected_clients.write().await.insert(client_id.clone());
    let _ = state.ui_tx.send(state.snapshot().await);

    let manifest = state.latest_manifest.read().await.clone();
    let initial_states = state.latest_states.read().await.clone();
    if send_wire(&mut sink, &WireMessage::Manifest(manifest))
        .await
        .is_err()
        || send_wire(
            &mut sink,
            &WireMessage::StateBatch {
                states: initial_states,
            },
        )
        .await
        .is_err()
    {
        state.connected_clients.write().await.remove(&client_id);
        return;
    }

    let mut rx = state.ha_tx.subscribe();

    loop {
        tokio::select! {
            broadcast_msg = rx.recv() => {
                match broadcast_msg {
                    Ok(msg) => {
                        if send_wire(&mut sink, &msg).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        match serde_json::from_str::<WireMessage>(&txt) {
                            Ok(WireMessage::Command { request_id, command }) => {
                                let res = state.execute_command(command, true).await;
                                let reply = match res {
                                    Ok(msg) => WireMessage::CommandResult {
                                        request_id,
                                        ok: true,
                                        message: Some(msg),
                                    },
                                    Err(err) => WireMessage::CommandResult {
                                        request_id,
                                        ok: false,
                                        message: Some(err),
                                    },
                                };
                                if send_wire(&mut sink, &reply).await.is_err() {
                                    break;
                                }
                            }
                            Ok(WireMessage::Ping { ts }) => {
                                if send_wire(&mut sink, &WireMessage::Pong { ts }).await.is_err() {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    info!("Home Assistant client ({client_id}) disconnected");
    state.connected_clients.write().await.remove(&client_id);
    let _ = state.ui_tx.send(state.snapshot().await);
}

async fn send_wire(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    msg: &WireMessage,
) -> Result<(), ()> {
    let txt = serde_json::to_string(msg).map_err(|_| ())?;
    sink.send(Message::Text(txt.into())).await.map_err(|_| ())
}

// --- Local IPC handlers for Tauri v2 Desktop App ---

async fn ipc_snapshot(State(state): State<Arc<AppState>>) -> Json<DaemonSnapshot> {
    Json(state.snapshot().await)
}

async fn ipc_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        let initial = state.snapshot().await;
        if let Ok(txt) = serde_json::to_string(&initial) {
            if socket.send(Message::Text(txt.into())).await.is_err() {
                return;
            }
        }
        let mut rx = state.ui_tx.subscribe();
        loop {
            tokio::select! {
                Ok(snap) = rx.recv() => {
                    if let Ok(txt) = serde_json::to_string(&snap) {
                        if socket.send(Message::Text(txt.into())).await.is_err() {
                            break;
                        }
                    }
                }
                msg = socket.recv() => {
                    if matches!(msg, None | Some(Ok(Message::Close(_)))) {
                        break;
                    }
                }
            }
        }
    })
}

async fn ipc_start_pairing(State(state): State<Arc<AppState>>) -> Json<DaemonSnapshot> {
    {
        let mut pm = state.pairing.lock().await;
        let sess = pm.start();
        info!(
            "Started 6-digit HA pairing session (expires at {})",
            sess.expires_at
        );
    }
    let snap = state.snapshot().await;
    let _ = state.ui_tx.send(snap.clone());
    Json(snap)
}

async fn ipc_cancel_pairing(State(state): State<Arc<AppState>>) -> Json<DaemonSnapshot> {
    {
        let mut pm = state.pairing.lock().await;
        pm.cancel();
    }
    let snap = state.snapshot().await;
    let _ = state.ui_tx.send(snap.clone());
    Json(snap)
}

async fn ipc_revoke_client(
    State(state): State<Arc<AppState>>,
    AxumPath(client_id): AxumPath<String>,
) -> Json<DaemonSnapshot> {
    {
        let mut cfg = state.config.write().await;
        cfg.paired_clients.retain(|c| c.client_id != client_id);
        let _ = cfg.save(&state.data_dir);
    }
    let snap = state.snapshot().await;
    let _ = state.ui_tx.send(snap.clone());
    Json(snap)
}

#[derive(Serialize)]
struct IpcCommandResponse {
    ok: bool,
    message: String,
}

async fn ipc_command(
    State(state): State<Arc<AppState>>,
    Json(cmd): Json<ControlCommand>,
) -> Json<IpcCommandResponse> {
    match state.execute_command(cmd, false).await {
        Ok(msg) => Json(IpcCommandResponse { ok: true, message: msg }),
        Err(err) => Json(IpcCommandResponse {
            ok: false,
            message: err,
        }),
    }
}

#[derive(Deserialize)]
struct UpdateSettingsRequest {
    permissions: Option<RemotePermissions>,
    polling_profile: Option<PollingProfile>,
    temperature_unit: Option<TemperatureUnit>,
    security_mode: Option<SecurityMode>,
    scripts: Option<Vec<AllowlistedScript>>,
    mqtt: Option<MqttConfig>,
}

async fn ipc_update_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateSettingsRequest>,
) -> Json<DaemonSnapshot> {
    let mut manifest_dirty = false;
    {
        let mut cfg = state.config.write().await;
        if let Some(p) = req.permissions {
            cfg.permissions = p;
        }
        if let Some(profile) = req.polling_profile {
            cfg.polling_profile = profile;
            state.poller.lock().await.set_profile(profile);
        }
        if let Some(unit) = req.temperature_unit {
            cfg.temperature_unit = unit;
        }
        if let Some(mode) = req.security_mode {
            cfg.security_mode = mode;
            state.collector.lock().await.set_security_mode(mode);
            manifest_dirty = true;
        }
        if let Some(scripts) = req.scripts {
            cfg.scripts = scripts;
            manifest_dirty = true;
        }
        if let Some(mqtt) = req.mqtt {
            cfg.mqtt = mqtt.clone();
            let _ = state.mqtt_cfg_tx.send(mqtt);
        }
        let _ = cfg.save(&state.data_dir);
    }

    if manifest_dirty {
        state.refresh_manifest().await;
    }

    let snap = state.snapshot().await;
    let _ = state.ui_tx.send(snap.clone());
    Json(snap)
}

async fn ipc_rescan_hardware(State(state): State<Arc<AppState>>) -> Json<DaemonSnapshot> {
    {
        let mut col = state.collector.lock().await;
        col.rescan_hardware();
    }
    state.refresh_manifest().await;
    Json(state.snapshot().await)
}
