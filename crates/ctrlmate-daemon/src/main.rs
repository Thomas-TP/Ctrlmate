mod auth;
mod config;
mod mdns;
mod mqtt;
mod server;
mod tls;
mod win_service;

use anyhow::Result;
use clap::Parser;
use ctrlmate_hal::smart_poll::SmartPoller;
use ctrlmate_hal::HardwareCollector;
use ctrlmate_protocol::{DEFAULT_HA_PORT, DEFAULT_IPC_PORT};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperBuilder;
use hyper_util::service::TowerToHyperService;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "ctrlmated",
    version,
    about = "Ctrlmate cross-platform PC daemon for Home Assistant 2026.11"
)]
struct Cli {
    /// Port for the TLS 1.3 HTTPS/WSS Home Assistant endpoint.
    #[arg(long, default_value_t = DEFAULT_HA_PORT)]
    ha_port: u16,

    /// Loopback port for the local Desktop UI IPC server.
    #[arg(long, default_value_t = DEFAULT_IPC_PORT)]
    ipc_port: u16,

    /// Optional override for the persistent config & TLS directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Print a one-shot hardware discovery & telemetry JSON snapshot and exit.
    #[arg(long)]
    print_telemetry: bool,

    /// Run under Windows Service Control Manager (SCM).
    #[arg(long)]
    service: bool,

    /// Install and start Ctrlmate as a native Windows Service (requires Administrator).
    #[arg(long)]
    install_service: bool,

    /// Stop and uninstall the Ctrlmate Windows Service (requires Administrator).
    #[arg(long)]
    uninstall_service: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    if cli.install_service {
        return win_service::install_windows_service(cli.ha_port, cli.data_dir);
    }
    if cli.uninstall_service {
        return win_service::uninstall_windows_service();
    }
    if cli.service {
        return win_service::run_as_windows_service(cli.ha_port, cli.ipc_port, cli.data_dir);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        if cli.print_telemetry {
            let sw_version = env!("CARGO_PKG_VERSION").to_string();
            let data_dir = config::DaemonConfig::resolve_data_dir(cli.data_dir)?;
            let cfg = config::DaemonConfig::load_or_create(&data_dir)?;
            let mut collector = HardwareCollector::new(
                cfg.device_id.clone(),
                sw_version,
                cfg.security_mode,
            );
            let manifest = collector.build_manifest(&cfg.scripts);
            let _ = collector.poll_states();
            tokio::time::sleep(Duration::from_millis(250)).await;
            let (fresh_states, _) = collector.poll_states();
            let out = serde_json::json!({
                "manifest": manifest,
                "states": fresh_states,
                "hid_devices": collector.hid_devices(),
                "rgb_devices": collector.rgb_device_statuses(),
                "conflicts": collector.detect_conflicts(),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        let (stop_tx, stop_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = stop_tx.send(true);
        });

        run_daemon(cli.ha_port, cli.ipc_port, cli.data_dir, stop_rx).await
    })
}

pub async fn run_daemon(
    ha_port: u16,
    ipc_port: u16,
    data_dir_override: Option<PathBuf>,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<()> {
    let sw_version = env!("CARGO_PKG_VERSION").to_string();
    let data_dir = config::DaemonConfig::resolve_data_dir(data_dir_override)?;
    let cfg = config::DaemonConfig::load_or_create(&data_dir)?;

    let mut collector = HardwareCollector::new(
        cfg.device_id.clone(),
        sw_version.clone(),
        cfg.security_mode,
    );
    let manifest = collector.build_manifest(&cfg.scripts);
    let (initial_states, _) = collector.poll_states();

    let tls_mat = tls::load_or_generate_tls(&data_dir, &manifest.host.name)?;
    info!(
        "Ctrlmate daemon v{sw_version} starting (device_id={}, host='{}', cert_fp={}...)",
        cfg.device_id,
        manifest.host.name,
        &tls_mat.cert_fingerprint[..16]
    );

    let _mdns = mdns::MdnsAnnouncer::start(
        &cfg.device_id,
        &manifest.host.name,
        ha_port,
        &sw_version,
        manifest.host.mac_address.as_deref(),
        &tls_mat.cert_fingerprint,
    );

    let (ha_tx, _) = broadcast::channel(64);
    let (ui_tx, _) = broadcast::channel(32);
    let (mqtt_cfg_tx, mqtt_cfg_rx) = watch::channel(cfg.mqtt.clone());

    let state = Arc::new(server::AppState {
        data_dir,
        ha_port,
        cert_fingerprint: tls_mat.cert_fingerprint,
        poller: Mutex::new(SmartPoller::new(cfg.polling_profile)),
        config: RwLock::new(cfg),
        collector: Mutex::new(collector),
        pairing: Mutex::new(auth::PairingManager::default()),
        latest_manifest: RwLock::new(manifest),
        latest_states: RwLock::new(initial_states),
        connected_clients: RwLock::new(HashSet::new()),
        ha_tx,
        ui_tx,
        mqtt_connected: AtomicBool::new(false),
        mqtt_cfg_tx,
    });

    // Optional Home Assistant 2026.11 MQTT Device + Child Device Discovery transport
    mqtt::spawn_mqtt_worker(Arc::clone(&state), mqtt_cfg_rx);

    // Background 1 Hz hardware poller task
    let poll_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            poll_state.tick_poll().await;
        }
    });

    // Background 60 FPS smooth lighting transition dispatcher (idle-friendly)
    let rgb_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let mut col = rgb_state.collector.lock().await;
            col.tick_lighting_transitions();
        }
    });

    // Background OS Update checker (initial scan 15s after start, then every 6 hours)
    let update_state = Arc::clone(&state);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        loop {
            update_state.spawn_update_scan();
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });

    // 1. Local IPC Server (127.0.0.1 only)
    let ipc_addr = SocketAddr::from(([127, 0, 0, 1], ipc_port));
    let ipc_listener = TcpListener::bind(ipc_addr).await?;
    let ipc_router = server::build_ipc_router(Arc::clone(&state));
    info!("Local Desktop IPC listening on http://{ipc_addr}");
    tokio::spawn(async move {
        if let Err(err) = axum::serve(ipc_listener, ipc_router).await {
            error!("IPC server stopped with error: {err}");
        }
    });

    // 2. LAN HTTPS/WSS Home Assistant Server (TLS 1.3 TOFU)
    let ha_addr = SocketAddr::from(([0, 0, 0, 0], ha_port));
    let ha_listener = TcpListener::bind(ha_addr).await?;
    let ha_router = server::build_ha_router(Arc::clone(&state));
    let tls_acceptor = TlsAcceptor::from(tls_mat.server_config);
    info!("Home Assistant WSS/HTTPS endpoint listening on https://{ha_addr}");

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    info!("Received shutdown signal — exiting ctrlmated cleanly");
                    break;
                }
            }
            accepted = ha_listener.accept() => {
                let (tcp_stream, peer) = match accepted {
                    Ok(v) => v,
                    Err(err) => {
                        warn!("TCP accept error: {err}");
                        continue;
                    }
                };
                let acceptor = tls_acceptor.clone();
                let router = ha_router.clone();

                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(tcp_stream).await {
                        Ok(s) => s,
                        Err(err) => {
                            warn!("TLS handshake failed from {peer}: {err}");
                            return;
                        }
                    };
                    let io = TokioIo::new(tls_stream);
                    let service = TowerToHyperService::new(router.into_service().map_request(
                        |req: hyper::Request<hyper::body::Incoming>| {
                            req.map(axum::body::Body::new)
                        },
                    ));
                    let builder = HyperBuilder::new(TokioExecutor::new());
                    if let Err(err) = builder.serve_connection_with_upgrades(io, service).await {
                        let msg = err.to_string();
                        if !msg.contains("connection closed") {
                            warn!("HTTPS/WSS connection error from {peer}: {msg}");
                        }
                    }
                });
            }
        }
    }

    Ok(())
}
