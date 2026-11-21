mod auth;
mod config;
mod mdns;
mod server;
mod tls;

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
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "ctrlmated", version, about = "Ctrlmate cross-platform PC daemon for Home Assistant 2026.11")]
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let sw_version = env!("CARGO_PKG_VERSION").to_string();
    let data_dir = config::DaemonConfig::resolve_data_dir(cli.data_dir)?;
    let cfg = config::DaemonConfig::load_or_create(&data_dir)?;

    let mut collector = HardwareCollector::new(
        cfg.device_id.clone(),
        sw_version.clone(),
        cfg.security_mode,
    );
    let manifest = collector.build_manifest(&cfg.scripts);
    let (initial_states, _) = collector.poll_states();

    if cli.print_telemetry {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let (fresh_states, _) = collector.poll_states();
        let out = serde_json::json!({
            "manifest": manifest,
            "states": fresh_states,
            "hid_devices": collector.hid_devices(),
            "conflicts": collector.detect_conflicts(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

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
        cli.ha_port,
        &sw_version,
        manifest.host.mac_address.as_deref(),
        &tls_mat.cert_fingerprint,
    );

    let (ha_tx, _) = broadcast::channel(64);
    let (ui_tx, _) = broadcast::channel(32);

    let state = Arc::new(server::AppState {
        data_dir,
        ha_port: cli.ha_port,
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
    });

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
    let ipc_addr = SocketAddr::from(([127, 0, 0, 1], cli.ipc_port));
    let ipc_listener = TcpListener::bind(ipc_addr).await?;
    let ipc_router = server::build_ipc_router(Arc::clone(&state));
    info!("Local Desktop IPC listening on http://{ipc_addr}");
    tokio::spawn(async move {
        if let Err(err) = axum::serve(ipc_listener, ipc_router).await {
            error!("IPC server stopped with error: {err}");
        }
    });

    // 2. LAN HTTPS/WSS Home Assistant Server (TLS 1.3 TOFU)
    let ha_addr = SocketAddr::from(([0, 0, 0, 0], cli.ha_port));
    let ha_listener = TcpListener::bind(ha_addr).await?;
    let ha_router = server::build_ha_router(Arc::clone(&state));
    let tls_acceptor = TlsAcceptor::from(tls_mat.server_config);
    info!("Home Assistant WSS/HTTPS endpoint listening on https://{ha_addr}");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received shutdown signal — exiting ctrlmated cleanly");
                break;
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
                    let service = TowerToHyperService::new(router.into_service().map_request(|req: hyper::Request<hyper::body::Incoming>| {
                        req.map(axum::body::Body::new)
                    }));
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
