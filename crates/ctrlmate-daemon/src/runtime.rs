//! Daemon lifecycle: startup order, background tasks and graceful shutdown.

use crate::config::DaemonConfig;
use crate::datadir::DataDir;
use crate::hw::{HwHandle, RealHardware};
use crate::ipc_token::TokenStore;
use crate::sessions::{AgentBackend, InProcessBackend, Sessions, overlay_session};
use crate::state::{AppState, RunMode, StateInit};
use crate::{ipc, lan, mdns, mqtt, notify, tls};
use anyhow::{Context, Result};
use ctrlmate_protocol::{
    AGENT_REQUEST_TIMEOUT_SECS, DataDirKind, EntityValue, IPC_WS_AUTH_TIMEOUT_SECS,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

pub struct RunOptions {
    pub ha_port: u16,
    pub ipc_port: u16,
    pub data_dir: Option<PathBuf>,
    /// Started as a Windows service / systemd unit (`--service`).
    pub service: bool,
    /// Allow the in-process session fallback when the daemon runs in a user session.
    pub allow_in_process: bool,
}

#[derive(Default)]
pub struct Hooks {
    /// Called once both listeners are bound and the background tasks run (service `Running`).
    pub on_ready: Option<Box<dyn FnOnce() + Send>>,
    /// Called when shutdown begins (service `StopPending`).
    pub on_stopping: Option<Box<dyn FnOnce() + Send>>,
}

const UPDATE_FIRST_SCAN: Duration = Duration::from_secs(15);
const UPDATE_SCAN_EVERY: Duration = Duration::from_secs(6 * 3600);
const MIN_POLL_SPACING: Duration = Duration::from_millis(250);

async fn sampler_loop(state: Arc<AppState>) {
    let mut last = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(Instant::now);
    loop {
        let interval = state.sampling_interval().await;
        let wait = interval.saturating_sub(last.elapsed());
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = state.sampler_wake.notified() => {
                // New listener or tier change: sample soon, but never in a tight loop.
                tokio::time::sleep(MIN_POLL_SPACING.saturating_sub(last.elapsed())).await;
            }
        }
        state.tick_poll().await;
        last = Instant::now();
    }
}

async fn update_checker(state: Arc<AppState>) {
    tokio::time::sleep(UPDATE_FIRST_SCAN).await;
    loop {
        let _ = state.spawn_update_scan();
        tokio::time::sleep(UPDATE_SCAN_EVERY).await;
    }
}

pub async fn run_daemon(
    opts: RunOptions,
    mut hooks: Hooks,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<()> {
    let sw_version = env!("CARGO_PKG_VERSION").to_string();
    let dd = DataDir::resolve(opts.data_dir, opts.service);
    let service_mode = opts.service || dd.kind == DataDirKind::System;
    dd.prepare()?;
    info!("Data directory: {} ({:?})", dd.path.display(), dd.kind);

    // Bind first: a busy port must fail fast, before any hardware is touched.
    let ipc_addr = SocketAddr::from(([127, 0, 0, 1], opts.ipc_port));
    let ipc_listener = TcpListener::bind(ipc_addr)
        .await
        .with_context(|| format!("cannot bind IPC {ipc_addr} (is another ctrlmated running?)"))?;
    let ha_addr = SocketAddr::from(([0, 0, 0, 0], opts.ha_port));
    let ha_listener = TcpListener::bind(ha_addr)
        .await
        .with_context(|| format!("cannot bind LAN endpoint {ha_addr}"))?;

    let cfg = DaemonConfig::load_or_create(&dd)?;
    let token = TokenStore::load(&dd)?;
    token.spawn_watcher();

    let hw = {
        let (id, ver) = (cfg.device_id.clone(), sw_version.clone());
        let opts = ctrlmate_hal::CollectorOptions {
            security_mode: cfg.security_mode,
            thermal: cfg.thermal,
            chassis_fan_ids: cfg.chassis_fan_ids.clone(),
            ..ctrlmate_hal::CollectorOptions::default()
        };
        HwHandle::spawn(move || RealHardware::new(id, ver, opts))
            .context("cannot start the hardware thread")?
    };
    let mut manifest = hw
        .build_manifest(cfg.scripts.clone())
        .await
        .context("hardware thread failed while building the manifest")?;
    crate::state::augment_manifest(&mut manifest, None);
    let first = hw
        .poll()
        .await
        .context("hardware thread failed during the first poll")?;

    let tls_mat = tls::load_or_generate_tls(&dd, &manifest.host.name)?;
    info!(
        "Ctrlmate daemon v{sw_version} starting (device_id={}, host='{}', cert_fp={}...)",
        cfg.device_id,
        manifest.host.name,
        &tls_mat.cert_fingerprint[..16]
    );
    let mdns = mdns::MdnsAnnouncer::start(
        &cfg.device_id,
        &manifest.host.name,
        opts.ha_port,
        &sw_version,
        &tls_mat.cert_fingerprint,
    );

    let agent = Arc::new(AgentBackend::new(Duration::from_secs(
        AGENT_REQUEST_TIMEOUT_SECS,
    )));
    let fallback =
        (!service_mode && opts.allow_in_process).then(|| Arc::new(InProcessBackend::new()));
    if fallback.is_some() {
        info!("Interactive run: session features run in-process until an agent connects");
    }
    let sessions = Sessions::new(agent, fallback);

    let mut states = first.states;
    overlay_session(&mut states, None);
    let requested = cfg.cooling_preset;
    if let Some(s) = states.iter_mut().find(|s| s.id == "cooling_preset") {
        s.value = EntityValue::Text(requested.as_ha_option().into());
    }
    let scripts = cfg.scripts.clone();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let state = AppState::new(StateInit {
        data_dir: dd,
        ha_port: opts.ha_port,
        ipc_port: opts.ipc_port,
        cert_fingerprint: tls_mat.cert_fingerprint,
        config: cfg,
        hw,
        manifest,
        states,
        hw_info: first.info,
        token,
        sessions,
        run: RunMode {
            service: service_mode,
        },
        shutdown: shutdown_tx,
        ws_auth_timeout: Duration::from_secs(IPC_WS_AUTH_TIMEOUT_SECS),
    });
    state.sessions.agent.push_scripts(&scripts);
    state.apply_cooling(true).await;

    let mut tasks: Vec<JoinHandle<()>> = Vec::new();
    mqtt::spawn_mqtt_worker(Arc::clone(&state), state.mqtt_cfg_tx.subscribe());
    tasks.push(tokio::spawn(sampler_loop(Arc::clone(&state))));
    tasks.push(tokio::spawn(update_checker(Arc::clone(&state))));

    // 1. Loopback IPC (desktop app + agent).
    let ipc_router = ipc::build_ipc_router(Arc::clone(&state));
    let mut ipc_shutdown = shutdown_rx.clone();
    let ipc_task = tokio::spawn(async move {
        let served = axum::serve(ipc_listener, ipc_router).with_graceful_shutdown(async move {
            let _ = ipc_shutdown.wait_for(|v| *v).await;
        });
        if let Err(err) = served.await {
            error!("IPC server stopped with error: {err}");
        }
    });
    info!("Local IPC listening on http://{ipc_addr}");

    // 2. LAN HTTPS/WSS endpoint for Home Assistant (TLS 1.3 only).
    let acceptor = TlsAcceptor::from(tls_mat.server_config);
    let lan_task = tokio::spawn(lan::serve_lan(
        Arc::clone(&state),
        ha_listener,
        acceptor,
        shutdown_rx.clone(),
    ));
    info!("Home Assistant TLS 1.3 endpoint listening on https://{ha_addr}");

    if let Some(ready) = hooks.on_ready.take() {
        ready();
    }
    notify::sd_notify("READY=1");

    // Wait for the stop request (Ctrl-C, SIGTERM, SCM stop).
    loop {
        if *stop_rx.borrow() {
            break;
        }
        if stop_rx.changed().await.is_err() {
            break;
        }
    }
    info!("Shutdown requested; stopping ctrlmated cleanly");
    if let Some(stopping) = hooks.on_stopping.take() {
        stopping();
    }
    notify::sd_notify("STOPPING=1");

    let _ = state.shutdown.send(true);
    for t in &tasks {
        t.abort();
    }
    for (name, task) in [("IPC", ipc_task), ("LAN", lan_task)] {
        if tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .is_err()
        {
            warn!("{name} server did not stop within 5 s");
        }
    }
    // Hands fans, pump, RGB and GPU back to firmware control (the collector's Drop).
    state.hw.shutdown().await;
    mdns.shutdown();
    info!("ctrlmated stopped");
    Ok(())
}
