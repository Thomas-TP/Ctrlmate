//! Shared daemon state, snapshot assembly and the periodic poll.

use crate::auth::PairingManager;
use crate::config::DaemonConfig;
use crate::datadir::DataDir;
use crate::hw::{HwHandle, HwInfo};
use crate::ipc_token::TokenStore;
use crate::sampler::{Listeners, Tier, sample_interval};
use crate::sensors::{self, SensorContext};
use crate::sessions::{SESSION_ENTITIES, SessionBackend, Sessions, overlay_session};
use chrono::Utc;
use ctrlmate_hal::smart_poll::SmartPoller;
use ctrlmate_protocol::{
    CommandResult, CoolingPolicy, CoolingPreset, CoolingReason, CoolingState, DaemonSnapshot,
    DeviceManifest, EntityCategory, EntityDescriptor, EntityPlatform, EntityState, EntityValue,
    MqttConfig, OsUpdateState, PROTOCOL_VERSION, PairedClientInfo, ResultCode, ServiceInfo,
    SessionState, SessionType, UpdatesInfo, WireMessage,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock, broadcast, watch};
use tracing::{debug, error, warn};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One live Home Assistant WebSocket.
pub struct ConnInfo {
    pub client_id: String,
    kill: watch::Sender<bool>,
}

/// Registry of live LAN connections, keyed by a per-connection id (a client may hold several).
#[derive(Default)]
pub struct Conns {
    next: AtomicU64,
    map: Mutex<HashMap<u64, ConnInfo>>,
}

impl Conns {
    /// Registers a connection; the receiver turns `true` when the connection must close
    /// (revoked token).
    pub fn register(&self, client_id: String) -> (u64, watch::Receiver<bool>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (kill, rx) = watch::channel(false);
        lock(&self.map).insert(id, ConnInfo { client_id, kill });
        (id, rx)
    }

    pub fn unregister(&self, id: u64) {
        lock(&self.map).remove(&id);
    }

    /// Closes every live connection of `client_id`; returns how many were signalled.
    pub fn revoke_client(&self, client_id: &str) -> usize {
        let map = lock(&self.map);
        let mut n = 0;
        for c in map.values().filter(|c| c.client_id == client_id) {
            let _ = c.kill.send(true);
            n += 1;
        }
        n
    }

    pub fn live_for(&self, client_id: &str) -> u32 {
        let n = lock(&self.map)
            .values()
            .filter(|c| c.client_id == client_id)
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        lock(&self.map).len()
    }
}

/// How the daemon was started.
#[derive(Debug, Clone, Copy)]
pub struct RunMode {
    /// Windows service / systemd unit.
    pub service: bool,
}

pub struct StateInit {
    pub data_dir: DataDir,
    pub ha_port: u16,
    pub ipc_port: u16,
    pub cert_fingerprint: String,
    pub config: DaemonConfig,
    pub hw: HwHandle,
    pub manifest: DeviceManifest,
    pub states: Vec<EntityState>,
    pub hw_info: HwInfo,
    pub token: Arc<TokenStore>,
    pub sessions: Sessions,
    pub run: RunMode,
    pub shutdown: watch::Sender<bool>,
    /// First-frame authentication window of the IPC WebSockets.
    pub ws_auth_timeout: Duration,
}

pub struct AppState {
    pub data_dir: DataDir,
    pub ha_port: u16,
    pub ipc_port: u16,
    pub cert_fingerprint: String,
    pub run: RunMode,
    pub config: RwLock<DaemonConfig>,
    pub hw: HwHandle,
    pub pairing: Mutex<PairingManager>,
    pub poller: Mutex<SmartPoller>,
    pub manifest: RwLock<DeviceManifest>,
    /// Current merged view: hardware states with the session data overlaid.
    pub states: RwLock<Vec<EntityState>>,
    pub hw_info: RwLock<HwInfo>,
    pub conns: Conns,
    /// Delta-filtered state batches and manifest updates for HA WebSockets and MQTT.
    pub ha_tx: broadcast::Sender<WireMessage>,
    /// Full snapshots for the desktop UI.
    pub ui_tx: broadcast::Sender<Arc<DaemonSnapshot>>,
    pub mqtt_connected: AtomicBool,
    pub mqtt_cfg_tx: watch::Sender<MqttConfig>,
    pub sessions: Sessions,
    pub ipc_token: Arc<TokenStore>,
    /// Wakes the sampler early (new listener, session change).
    pub sampler_wake: Notify,
    /// Mirror of `permissions.telemetry` for lock-free checks in the broadcast consumers.
    pub telemetry: AtomicBool,
    /// An OS update scan or install is running.
    pub update_busy: AtomicBool,
    update_error: Mutex<Option<String>>,
    applied_cooling: Mutex<Option<CoolingPreset>>,
    last_audio_outputs: Mutex<Vec<String>>,
    last_tier: Mutex<Tier>,
    /// Closes WebSockets and servers on shutdown.
    pub shutdown: watch::Sender<bool>,
    pub ws_auth_timeout: Duration,
}

/// `auto` becomes `gaming` while the agent reports a game; every other preset is literal.
pub fn effective_preset(requested: CoolingPreset, session: Option<&SessionState>) -> CoolingPreset {
    if requested == CoolingPreset::Auto && session.is_some_and(|s| s.gaming_mode && !s.locked) {
        CoolingPreset::Gaming
    } else {
        requested
    }
}

/// Result code of a failed OS update operation.
fn update_error_code(e: &ctrlmate_hal::updates::UpdateError) -> &'static str {
    use ctrlmate_hal::updates::UpdateError;
    match e {
        UpdateError::Busy => ResultCode::Busy.as_str(),
        UpdateError::Unavailable(_) => ResultCode::Unavailable.as_str(),
        UpdateError::Failed(_) => ResultCode::HardwareError.as_str(),
    }
}

/// Diagnostic binary sensor telling Home Assistant whether the session agent is connected.
pub const AGENT_ENTITY: &str = "agent_connected";

/// Adds the daemon-owned entities (not produced by the hardware layer) to a manifest.
pub fn augment_manifest(m: &mut DeviceManifest, session: Option<&SessionState>) {
    // The audio output select lists what the agent reports (the service itself has no audio session).
    let outputs: Vec<String> = session
        .and_then(|s| s.audio.as_ref())
        .map(|a| a.outputs.iter().map(|o| o.name.clone()).collect())
        .unwrap_or_default();
    if let Some(e) = m.entities.iter_mut().find(|e| e.id == "audio_output") {
        e.options = outputs;
    }
    if m.entities.iter().any(|e| e.id == AGENT_ENTITY) {
        return;
    }
    m.entities.push(EntityDescriptor {
        id: AGENT_ENTITY.into(),
        name: "Agent connected".into(),
        platform: EntityPlatform::BinarySensor,
        child_device_id: None,
        device_class: Some("connectivity".into()),
        state_class: None,
        unit_of_measurement: None,
        icon: Some("mdi:account-network".into()),
        entity_category: Some(EntityCategory::Diagnostic),
        suggested_display_precision: None,
        options: vec![],
        number_range: None,
        light_caps: None,
        exposed: true,
    });
}

impl AppState {
    pub fn new(init: StateInit) -> Arc<Self> {
        let (ha_tx, _) = broadcast::channel(256);
        let (ui_tx, _) = broadcast::channel(16);
        let (mqtt_cfg_tx, _) = watch::channel(init.config.mqtt.clone());
        Arc::new(Self {
            data_dir: init.data_dir,
            ha_port: init.ha_port,
            ipc_port: init.ipc_port,
            cert_fingerprint: init.cert_fingerprint,
            run: init.run,
            telemetry: AtomicBool::new(init.config.permissions.telemetry),
            poller: Mutex::new(SmartPoller::new(init.config.polling_profile)),
            config: RwLock::new(init.config),
            hw: init.hw,
            pairing: Mutex::new(PairingManager::default()),
            manifest: RwLock::new(init.manifest),
            states: RwLock::new(init.states),
            hw_info: RwLock::new(init.hw_info),
            conns: Conns::default(),
            ha_tx,
            ui_tx,
            mqtt_connected: AtomicBool::new(false),
            mqtt_cfg_tx,
            sessions: init.sessions,
            ipc_token: init.token,
            sampler_wake: Notify::new(),
            update_busy: AtomicBool::new(false),
            update_error: Mutex::new(None),
            applied_cooling: Mutex::new(None),
            last_audio_outputs: Mutex::new(Vec::new()),
            last_tier: Mutex::new(Tier::Active),
            shutdown: init.shutdown,
            ws_auth_timeout: init.ws_auth_timeout,
        })
    }

    pub fn telemetry_allowed(&self) -> bool {
        self.telemetry.load(Ordering::Relaxed)
    }

    pub async fn snapshot(&self) -> DaemonSnapshot {
        let manifest = self.manifest.read().await.clone();
        let states = self.states.read().await.clone();
        let info = self.hw_info.read().await.clone();
        let cfg = self.config.read().await.clone();
        let session = self.sessions.state();

        let pairing = lock(&self.pairing).status(&self.cert_fingerprint, self.ha_port);

        let paired_clients = cfg
            .paired_clients
            .iter()
            .map(|c| {
                let connections = self.conns.live_for(&c.client_id);
                PairedClientInfo {
                    client_id: c.client_id.clone(),
                    client_name: c.client_name.clone(),
                    paired_at: c.paired_at,
                    connected: connections > 0,
                    connections,
                }
            })
            .collect();

        // Curves, state and reason come from the cooling watchdog itself; the requested / effective
        // preset and the thresholds are the daemon's (config) view, so they update immediately.
        let mut policy: CoolingPolicy = info.cooling.clone();
        policy.requested_preset = cfg.cooling_preset;
        policy.effective_preset = effective_preset(cfg.cooling_preset, session.as_ref());
        policy.thresholds = cfg.thermal;
        if info.failsafe_active && policy.state != CoolingState::Failsafe {
            policy.state = CoolingState::Failsafe;
            policy.reason.get_or_insert(CoolingReason::Temperature);
        }

        let sensor_status = sensors::compute(&SensorContext {
            states: &states,
            manifest: &manifest,
            mode: cfg.security_mode,
            deep_bridge_active: info.deep_bridge_active,
            conflicts: &info.conflicts,
            windows: cfg!(windows),
        });

        let mut updates = states
            .iter()
            .find_map(|s| match &s.value {
                EntityValue::Update(u) if s.id == "os_updates" => Some(u.clone()),
                _ => None,
            })
            .unwrap_or_default();
        updates.error = lock(&self.update_error).clone();

        let mut mqtt = cfg.mqtt.clone();
        let mqtt_password_set = mqtt.password.as_deref().is_some_and(|p| !p.is_empty());
        mqtt.password = None;

        DaemonSnapshot {
            manifest,
            states,
            conflicts: info.conflicts,
            hid_devices: info.hid_devices,
            pairing,
            paired_clients,
            permissions: cfg.permissions,
            polling_profile: cfg.polling_profile,
            temperature_unit: cfg.temperature_unit,
            scripts: cfg.scripts,
            thermal_failsafe_active: info.failsafe_active,
            rgb_backends: info.rgb_backends,
            rgb_devices: info.rgb_devices,
            deep_bridge_active: info.deep_bridge_active,
            mqtt,
            mqtt_connected: self.mqtt_connected.load(Ordering::Relaxed),
            mqtt_password_set,
            cooling_preset: cfg.cooling_preset,
            cooling_policy: policy,
            sensor_status,
            session,
            service: self.service_info(),
            updates,
            include_optional_updates: cfg.include_optional_updates,
        }
    }

    pub fn service_info(&self) -> ServiceInfo {
        let hello = self.sessions.agent.hello();
        ServiceInfo {
            agent_version: hello.as_ref().map(|h| h.agent_version.clone()),
            agent_session_id: hello.map(|h| h.session_id),
            running_as_service: self.run.service,
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            session_type: if self.run.service {
                SessionType::Service
            } else {
                SessionType::Interactive
            },
            agent_connected: self.sessions.agent_connected(),
            in_process_session: self.sessions.in_process_active(),
            data_dir_kind: self.data_dir.kind,
        }
    }

    /// Pushes a fresh snapshot to the desktop UI when somebody is listening.
    pub async fn notify_ui(&self) {
        if self.ui_tx.receiver_count() > 0 {
            let snap = self.snapshot().await;
            let _ = self.ui_tx.send(Arc::new(snap));
        }
    }

    /// Manifest followed by every current state, used to (re)synchronise a consumer that
    /// connected late or lagged behind the broadcast.
    pub async fn resync_frames(&self) -> (WireMessage, WireMessage) {
        let manifest = self.manifest.read().await.clone();
        let states = self.states.read().await.clone();
        (
            WireMessage::Manifest(manifest),
            WireMessage::StateBatch { states },
        )
    }

    /// Rebuilds the manifest (script allowlist, audio outputs, topology) and pushes it.
    pub async fn refresh_manifest(&self) {
        let scripts = self.config.read().await.scripts.clone();
        let Some(mut manifest) = self.hw.build_manifest(scripts).await else {
            error!("Hardware thread unavailable: manifest not refreshed");
            return;
        };
        augment_manifest(&mut manifest, self.sessions.state().as_ref());
        *self.manifest.write().await = manifest.clone();
        let _ = self.ha_tx.send(WireMessage::Manifest(manifest));
        self.notify_ui().await;
    }

    /// Merges `batch` into the current states and broadcasts it. HA / MQTT consumers apply the
    /// `telemetry` permission themselves.
    pub async fn merge_and_broadcast_states(&self, batch: Vec<EntityState>) {
        {
            let mut latest = self.states.write().await;
            for item in &batch {
                if let Some(existing) = latest.iter_mut().find(|s| s.id == item.id) {
                    *existing = item.clone();
                } else {
                    latest.push(item.clone());
                }
            }
        }
        let _ = self.ha_tx.send(WireMessage::StateBatch { states: batch });
        self.notify_ui().await;
    }

    /// Sets the `agent_connected` diagnostic entity.
    fn put_agent_flag(&self, states: &mut Vec<EntityState>) {
        let value = EntityValue::Bool(self.sessions.agent_connected());
        if let Some(s) = states.iter_mut().find(|s| s.id == AGENT_ENTITY) {
            if s.value != value {
                s.value = value;
                s.updated_at = Utc::now();
            }
        } else {
            states.push(EntityState {
                id: AGENT_ENTITY.into(),
                value,
                attributes: Default::default(),
                updated_at: Utc::now(),
            });
        }
    }

    /// Applies the effective cooling preset to the hardware thread (idempotent).
    pub async fn apply_cooling(&self, force: bool) {
        let requested = self.config.read().await.cooling_preset;
        let eff = effective_preset(requested, self.sessions.state().as_ref());
        let mut applied = lock(&self.applied_cooling);
        if force || *applied != Some(eff) {
            *applied = Some(eff);
            drop(applied);
            self.hw.set_cooling_preset(eff);
        }
    }

    /// Called whenever the session state changed (agent update, connect, disconnect).
    pub async fn on_session_changed(&self) {
        let session = self.sessions.state();
        self.apply_cooling(false).await;

        let changed: Vec<EntityState> = {
            let mut states = self.states.write().await;
            let before = states.clone();
            overlay_session(&mut states, session.as_ref());
            self.put_agent_flag(&mut states);
            SESSION_ENTITIES
                .iter()
                .copied()
                .chain([AGENT_ENTITY])
                .filter_map(|id| {
                    let new = states.iter().find(|s| s.id == id)?;
                    let old = before.iter().find(|s| s.id == id);
                    (old.is_none_or(|o| o.value != new.value || o.attributes != new.attributes))
                        .then(|| new.clone())
                })
                .collect()
        };
        if !changed.is_empty() {
            let _ = self.ha_tx.send(WireMessage::StateBatch { states: changed });
        }
        // The audio output select lists the agent's outputs: republish the manifest when they change.
        let outputs: Vec<String> = session
            .as_ref()
            .and_then(|s| s.audio.as_ref())
            .map(|a| a.outputs.iter().map(|o| o.name.clone()).collect())
            .unwrap_or_default();
        let outputs_changed = {
            let mut last = lock(&self.last_audio_outputs);
            let changed = *last != outputs;
            *last = outputs;
            changed
        };
        if outputs_changed {
            self.refresh_manifest().await;
        }
        self.notify_ui().await;
        let tier = Tier::from_session(session.as_ref());
        let tier_changed = {
            let mut last = lock(&self.last_tier);
            let changed = *last != tier;
            *last = tier;
            changed
        };
        if tier_changed {
            self.sampler_wake.notify_one();
        }
    }

    /// Current sampling interval for the situation (see `sampler`).
    pub async fn sampling_interval(&self) -> Duration {
        let profile = self.config.read().await.polling_profile;
        let tier = Tier::from_session(self.sessions.state().as_ref());
        sample_interval(
            profile,
            tier,
            Listeners {
                ui: self.ui_tx.receiver_count() > 0,
                remote: self.ha_tx.receiver_count() > 0,
            },
        )
    }

    /// One hardware poll: never holds a lock across the blocking work.
    pub async fn tick_poll(&self) {
        if let Some(fb) = self
            .sessions
            .fallback
            .as_ref()
            .filter(|_| !self.sessions.agent_connected())
        {
            fb.refresh().await;
        }

        let Some(out) = self.hw.poll().await else {
            warn!("Hardware thread did not answer the poll");
            return;
        };
        *self.hw_info.write().await = out.info;
        if out.manifest_dirty {
            self.refresh_manifest().await;
        }

        let session = self.sessions.state();
        let requested = self.config.read().await.cooling_preset;
        let mut states = out.states;
        overlay_session(&mut states, session.as_ref());
        self.put_agent_flag(&mut states);
        // The select entity shows what the user asked for; failsafe is a separate entity.
        if let Some(s) = states.iter_mut().find(|s| s.id == "cooling_preset") {
            s.value = EntityValue::Text(requested.as_ha_option().into());
        }
        let update_error = lock(&self.update_error).clone();
        if let Some(s) = states.iter_mut().find(|s| s.id == "os_updates")
            && let EntityValue::Update(u) = &mut s.value
        {
            u.error = update_error;
        }
        {
            let mut cur = self.states.write().await;
            // A sensor that was reported before and is gone now becomes explicitly unavailable
            // (never a stale or fabricated value) and stays listed until it reappears.
            let now = Utc::now();
            let gone: Vec<EntityState> = cur
                .iter()
                .filter(|old| !states.iter().any(|s| s.id == old.id))
                .map(|old| {
                    if old.value == EntityValue::Unavailable {
                        old.clone()
                    } else {
                        EntityState {
                            id: old.id.clone(),
                            value: EntityValue::Unavailable,
                            attributes: Default::default(),
                            updated_at: now,
                        }
                    }
                })
                .collect();
            states.extend(gone);
            cur.clone_from(&states);
        }

        self.notify_ui().await;

        if self.ha_tx.receiver_count() > 0 {
            let tier = Tier::from_session(session.as_ref()).publish_tier();
            let filtered = lock(&self.poller).filter_batch(&states, tier, Instant::now());
            if !filtered.is_empty() {
                debug!(
                    "Pushing {} delta-filtered entity updates (tier={tier:?})",
                    filtered.len()
                );
                let _ = self
                    .ha_tx
                    .send(WireMessage::StateBatch { states: filtered });
            }
        }
    }

    // --- OS updates -------------------------------------------------------------------------

    async fn update_policy(&self) -> ctrlmate_hal::updates::UpdatePolicy {
        ctrlmate_hal::updates::UpdatePolicy {
            include_optional: self.config.read().await.include_optional_updates,
        }
    }

    fn set_update_error(&self, e: Option<&str>) {
        *lock(&self.update_error) = e.map(str::to_string);
    }

    fn update_batch(&self, upd: &OsUpdateState) -> Vec<EntityState> {
        let now = Utc::now();
        let mut info = UpdatesInfo::from(upd);
        info.error = lock(&self.update_error).clone();
        vec![
            EntityState {
                id: "os_updates".into(),
                value: EntityValue::Update(info),
                attributes: Default::default(),
                updated_at: now,
            },
            EntityState {
                id: "reboot_required".into(),
                value: EntityValue::Bool(upd.reboot_required),
                attributes: Default::default(),
                updated_at: now,
            },
        ]
    }

    /// Starts a background WUA / PackageKit scan.
    pub fn spawn_update_scan(self: &Arc<Self>) -> CommandResult {
        if self.update_busy.swap(true, Ordering::AcqRel) {
            return CommandResult::err(ResultCode::Busy);
        }
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let policy = state.update_policy().await;
            match tokio::task::spawn_blocking(move || {
                ctrlmate_hal::updates::check_os_updates(policy)
            })
            .await
            {
                Ok(Ok(report)) => {
                    let upd = report.state;
                    state.set_update_error(None);
                    state.hw.set_os_updates(upd.clone());
                    state
                        .merge_and_broadcast_states(state.update_batch(&upd))
                        .await;
                }
                Ok(Err(e)) => {
                    // A failed scan is an error state, never "0 pending".
                    warn!("OS update scan failed: {e}");
                    state.set_update_error(Some(update_error_code(&e)));
                    state.notify_ui().await;
                }
                Err(e) => {
                    warn!("OS update scan task failed: {e}");
                    state.set_update_error(Some(ResultCode::Internal.as_str()));
                    state.notify_ui().await;
                }
            }
            state.update_busy.store(false, Ordering::Release);
        });
        CommandResult::ok()
    }

    /// Starts a background download + installation.
    pub fn spawn_update_install(self: &Arc<Self>) -> CommandResult {
        if self.update_busy.swap(true, Ordering::AcqRel) {
            return CommandResult::err(ResultCode::Busy);
        }
        let state = Arc::clone(self);
        tokio::spawn(async move {
            if let Some(prog) = state.hw.set_os_updates_in_progress(true).await {
                state
                    .merge_and_broadcast_states(state.update_batch(&prog))
                    .await;
            }
            let policy = state.update_policy().await;
            match tokio::task::spawn_blocking(move || {
                ctrlmate_hal::updates::install_os_updates(policy)
            })
            .await
            {
                Ok(Ok(report)) => {
                    let fin = report.state;
                    state.set_update_error(None);
                    state.hw.set_os_updates(fin.clone());
                    state
                        .merge_and_broadcast_states(state.update_batch(&fin))
                        .await;
                }
                Ok(Err(err)) => {
                    warn!("OS update install error: {err}");
                    state.set_update_error(Some(update_error_code(&err)));
                    if let Some(rev) = state.hw.set_os_updates_in_progress(false).await {
                        state
                            .merge_and_broadcast_states(state.update_batch(&rev))
                            .await;
                    }
                }
                Err(err) => {
                    warn!("OS update install task panicked: {err}");
                    state.set_update_error(Some(ResultCode::Internal.as_str()));
                    if let Some(rev) = state.hw.set_os_updates_in_progress(false).await {
                        state
                            .merge_and_broadcast_states(state.update_batch(&rev))
                            .await;
                    }
                }
            }
            state.update_busy.store(false, Ordering::Release);
        });
        CommandResult::ok()
    }

    /// Removes a paired client and closes its live sockets immediately.
    pub async fn revoke_client(&self, client_id: &str) -> bool {
        let found = {
            let mut cfg = self.config.write().await;
            let before = cfg.paired_clients.len();
            cfg.paired_clients.retain(|c| c.client_id != client_id);
            let found = cfg.paired_clients.len() != before;
            if found && let Err(e) = cfg.save(&self.data_dir) {
                error!("Failed persisting client revocation: {e:#}");
            }
            found
        };
        if found {
            let closed = self.conns.revoke_client(client_id);
            tracing::info!("Revoked client {client_id}; closing {closed} live connection(s)");
            self.notify_ui().await;
        }
        found
    }
}

#[cfg(test)]
pub mod testutil {
    use super::*;
    use crate::hw::fake::{FakeLog, spawn_fake};
    use crate::sessions::{AgentBackend, InProcessBackend};
    use ctrlmate_protocol::DataDirKind;

    pub struct TestRig {
        pub state: Arc<AppState>,
        pub log: Arc<Mutex<FakeLog>>,
        pub token: String,
        _dir: tempfile::TempDir,
    }

    pub struct RigOpts {
        pub fallback: bool,
        pub agent_timeout: Duration,
        pub ipc_port: u16,
        pub ws_auth_timeout: Duration,
    }

    impl Default for RigOpts {
        fn default() -> Self {
            Self {
                fallback: false,
                agent_timeout: Duration::from_millis(300),
                ipc_port: 9488,
                ws_auth_timeout: Duration::from_millis(400),
            }
        }
    }

    pub async fn rig_with(fallback: bool, agent_timeout: Duration) -> TestRig {
        rig_opts(RigOpts {
            fallback,
            agent_timeout,
            ..RigOpts::default()
        })
        .await
    }

    pub async fn rig_opts(o: RigOpts) -> TestRig {
        let (fallback, agent_timeout) = (o.fallback, o.agent_timeout);
        let dir = tempfile::tempdir().expect("tempdir");
        let dd = DataDir {
            path: dir.path().to_path_buf(),
            kind: DataDirKind::Custom,
        };
        let cfg = DaemonConfig::load_or_create(&dd).expect("config");
        let token_store = TokenStore::load(&dd).expect("token");
        let token =
            std::fs::read_to_string(dd.file(crate::ipc_token::TOKEN_FILE)).expect("token file");
        let (hw, log) = spawn_fake();
        let mut manifest = hw.build_manifest(vec![]).await.expect("manifest");
        augment_manifest(&mut manifest, None);
        let out = hw.poll().await.expect("poll");
        let mut states = out.states;
        overlay_session(&mut states, None);
        let (shutdown, _) = watch::channel(false);
        let sessions = Sessions::new(
            Arc::new(AgentBackend::new(agent_timeout)),
            fallback.then(|| Arc::new(InProcessBackend::new())),
        );
        let state = AppState::new(StateInit {
            data_dir: dd,
            ha_port: 9487,
            ipc_port: o.ipc_port,
            cert_fingerprint: "ab".repeat(32),
            config: cfg,
            hw,
            manifest,
            states,
            hw_info: out.info,
            token: token_store,
            sessions,
            run: RunMode { service: false },
            shutdown,
            ws_auth_timeout: o.ws_auth_timeout,
        });
        TestRig {
            state,
            log,
            token: token.trim().to_string(),
            _dir: dir,
        }
    }

    pub async fn rig() -> TestRig {
        rig_with(false, Duration::from_millis(300)).await
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn auto_becomes_gaming_only_while_gaming_and_unlocked() {
        let game = SessionState {
            gaming_mode: true,
            ..SessionState::default()
        };
        assert_eq!(
            effective_preset(CoolingPreset::Auto, Some(&game)),
            CoolingPreset::Gaming
        );
        assert_eq!(
            effective_preset(CoolingPreset::Auto, None),
            CoolingPreset::Auto
        );
        assert_eq!(
            effective_preset(CoolingPreset::Silent, Some(&game)),
            CoolingPreset::Silent
        );
        let locked = SessionState {
            gaming_mode: true,
            locked: true,
            ..SessionState::default()
        };
        assert_eq!(
            effective_preset(CoolingPreset::Auto, Some(&locked)),
            CoolingPreset::Auto
        );
    }

    #[test]
    fn updates_info_has_no_fake_counts() {
        let fresh = OsUpdateState {
            installed_version: "Windows 11".into(),
            latest_version: "Windows 11".into(),
            ..OsUpdateState::default()
        };
        let info = UpdatesInfo::from(&fresh);
        assert!(!info.checked);
        assert_eq!(info.pending_count, None);

        let scanned = OsUpdateState {
            pending_count: 2,
            pending_titles: vec![
                "Cumulative Update (KB5050009)".into(),
                "Driver update".into(),
            ],
            last_checked: Some(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap()),
            reboot_required: true,
            ..fresh
        };
        let info = UpdatesInfo::from(&scanned);
        assert!(info.checked);
        assert_eq!(info.pending_count, Some(2));
        assert_eq!(info.kb_ids, vec!["KB5050009".to_string()]);
        assert!(info.pending[1].kb.is_none());
        assert!(info.reboot_required);
    }

    #[test]
    fn conns_are_keyed_by_connection_id_and_revocation_signals_all() {
        let conns = Conns::default();
        let (a, mut ra) = conns.register("c1".into());
        let (b, rb) = conns.register("c1".into());
        let (_c, rc) = conns.register("c2".into());
        assert_ne!(a, b);
        assert_eq!(conns.live_for("c1"), 2);
        assert_eq!(conns.revoke_client("c1"), 2);
        assert!(*ra.borrow_and_update());
        assert!(*rb.borrow());
        assert!(!*rc.borrow());
        conns.unregister(a);
        assert_eq!(conns.live_for("c1"), 1);
        assert_eq!(conns.len(), 2);
    }

    #[tokio::test]
    async fn snapshot_merges_session_cooling_and_service_info() {
        let rig = rig().await;
        let snap = rig.state.snapshot().await;
        assert!(snap.session.is_none());
        assert!(!snap.service.agent_connected);
        assert_eq!(snap.cooling_policy.presets.len(), 5);
        assert_eq!(snap.service.version, env!("CARGO_PKG_VERSION"));
        // No agent: session entities are unavailable, never fabricated.
        let vol = snap
            .states
            .iter()
            .find(|s| s.id == "master_volume")
            .unwrap();
        assert_eq!(vol.value, EntityValue::Unavailable);
        assert_eq!(snap.updates.pending_count, None);
        assert!(snap.mqtt.password.is_none());
        assert!(snap.sensor_status.contains_key("cpu_0_temperature"));
    }

    #[tokio::test]
    async fn agent_state_flows_into_snapshot_cooling_and_sampling() {
        let rig = rig().await;
        let st = &rig.state;
        let att = st.sessions.agent.attach();
        st.sessions.agent.set_state(
            att.conn_id,
            SessionState {
                gaming_mode: true,
                mic_in_use: Some(true),
                ..SessionState::default()
            },
        );
        let mut ha = st.ha_tx.subscribe();
        st.on_session_changed().await;
        let snap = st.snapshot().await;
        assert!(snap.service.agent_connected);
        assert!(snap.session.as_ref().is_some_and(|s| s.gaming_mode));
        assert_eq!(snap.cooling_policy.effective_preset, CoolingPreset::Gaming);
        assert_eq!(snap.cooling_policy.requested_preset, CoolingPreset::Auto);
        // The hardware thread was told to use the gaming curve.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            rig.log
                .lock()
                .unwrap()
                .calls
                .contains(&"preset:gaming".to_string())
        );
        // Instant entities were pushed to HA.
        let Ok(WireMessage::StateBatch { states }) = ha.try_recv() else {
            panic!("expected state batch");
        };
        assert!(
            states
                .iter()
                .any(|s| s.id == "gaming_mode" && s.value == EntityValue::Bool(true))
        );
        assert_eq!(st.sampling_interval().await, Duration::from_secs(2));
    }

    #[tokio::test]
    async fn tick_poll_overlays_session_and_publishes_deltas_only_to_listeners() {
        let rig = rig().await;
        let st = &rig.state;
        // No listeners: nothing is broadcast, states still refresh.
        st.tick_poll().await;
        let mut ha = st.ha_tx.subscribe();
        st.tick_poll().await;
        // First batch for a new listener contains everything the poller has never sent.
        let Ok(WireMessage::StateBatch { states }) = ha.try_recv() else {
            panic!("expected batch");
        };
        assert!(!states.is_empty());
        // Identical second poll is fully suppressed by the delta filter.
        st.tick_poll().await;
        assert!(ha.try_recv().is_err());
        let cooling = st
            .states
            .read()
            .await
            .iter()
            .find(|s| s.id == "cooling_preset")
            .cloned();
        assert!(
            cooling.is_none()
                || cooling.is_some_and(|c| c.value == EntityValue::Text("auto".into()))
        );
    }

    #[tokio::test]
    async fn poll_does_not_hold_state_locks_while_hardware_is_slow() {
        let rig = rig().await;
        rig.log.lock().unwrap().poll_delay = Duration::from_millis(500);
        let st = Arc::clone(&rig.state);
        let t = tokio::spawn(async move { st.tick_poll().await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        // While the hardware thread blocks in poll(), readers and writers proceed immediately.
        let started = Instant::now();
        let _ = rig.state.snapshot().await;
        let _ = rig.state.config.write().await;
        let _ = rig.state.states.write().await;
        assert!(started.elapsed() < Duration::from_millis(200));
        t.await.unwrap();
    }

    #[tokio::test]
    async fn revoke_removes_client_and_signals_connections() {
        let rig = rig().await;
        let st = &rig.state;
        let (tok, stored) = crate::auth::issue_client_token("HA");
        let _ = tok;
        let id = stored.client_id.clone();
        st.config.write().await.paired_clients.push(stored);
        let (_cid, mut kill) = st.conns.register(id.clone());
        assert!(st.revoke_client(&id).await);
        assert!(*kill.borrow_and_update());
        assert!(!st.revoke_client(&id).await);
        assert!(st.config.read().await.paired_clients.is_empty());
    }

    #[tokio::test]
    async fn manifest_refresh_broadcasts() {
        let rig = rig().await;
        let mut ha = rig.state.ha_tx.subscribe();
        rig.state.refresh_manifest().await;
        assert!(matches!(ha.try_recv(), Ok(WireMessage::Manifest(_))));
    }

    #[tokio::test]
    async fn update_batch_has_both_entities_and_honest_payload() {
        let rig = rig().await;
        let b = rig.state.update_batch(&OsUpdateState::default());
        assert_eq!(b.len(), 2);
        let EntityValue::Update(u) = &b[0].value else {
            panic!("update value expected");
        };
        assert_eq!(u.pending_count, None);
    }

    #[tokio::test]
    async fn disappeared_sensor_becomes_unavailable_and_agent_flag_follows_the_agent() {
        let rig = rig().await;
        let st = &rig.state;
        st.tick_poll().await;
        let find = |states: &[EntityState], id: &str| {
            states.iter().find(|s| s.id == id).map(|s| s.value.clone())
        };
        assert_eq!(
            find(&st.states.read().await, "agent_connected"),
            Some(EntityValue::Bool(false))
        );
        assert_eq!(
            find(&st.states.read().await, "gpu_0_temperature"),
            Some(EntityValue::Float(51.0))
        );
        rig.log.lock().unwrap().hide = vec!["gpu_0_temperature".into()];
        st.tick_poll().await;
        assert_eq!(
            find(&st.states.read().await, "gpu_0_temperature"),
            Some(EntityValue::Unavailable)
        );
        // Still unavailable (and listed) on the following poll.
        st.tick_poll().await;
        assert_eq!(
            find(&st.states.read().await, "gpu_0_temperature"),
            Some(EntityValue::Unavailable)
        );
        let att = st.sessions.agent.attach();
        st.on_session_changed().await;
        assert_eq!(
            find(&st.states.read().await, "agent_connected"),
            Some(EntityValue::Bool(true))
        );
        st.sessions.agent.detach(att.conn_id);
        st.on_session_changed().await;
        assert_eq!(
            find(&st.states.read().await, "agent_connected"),
            Some(EntityValue::Bool(false))
        );
    }

    #[tokio::test]
    async fn manifest_carries_the_agent_diagnostic_sensor() {
        let rig = rig().await;
        let m = rig.state.manifest.read().await;
        let e = m
            .entities
            .iter()
            .find(|e| e.id == "agent_connected")
            .expect("entity");
        assert_eq!(e.platform, EntityPlatform::BinarySensor);
        assert!(e.child_device_id.is_none());
        assert_eq!(e.entity_category, Some(EntityCategory::Diagnostic));
    }
}
