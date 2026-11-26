mod auth;
mod commands;
mod config;
mod datadir;
mod hw;
mod ipc;
mod ipc_token;
mod lan;
mod mdns;
mod mqtt;
mod notify;
mod runtime;
mod sampler;
mod scripts;
mod sensors;
mod sessions;
mod state;
mod tls;
mod win_service;

use anyhow::Result;
use clap::Parser;
use ctrlmate_hal::HardwareCollector;
use ctrlmate_protocol::{DEFAULT_HA_PORT, DEFAULT_IPC_PORT};
use runtime::{Hooks, RunOptions};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "ctrlmated",
    version,
    about = "Ctrlmate PC daemon: hardware, cooling, lighting and Home Assistant bridge"
)]
struct Cli {
    /// Port of the TLS 1.3 HTTPS/WSS endpoint for Home Assistant (LAN).
    #[arg(long, default_value_t = DEFAULT_HA_PORT)]
    ha_port: u16,

    /// Loopback port of the IPC server (desktop app and agent).
    #[arg(long, default_value_t = DEFAULT_IPC_PORT)]
    ipc_port: u16,

    /// Override the data directory (default: `%ProgramData%\Ctrlmate` / `/var/lib/ctrlmate` for a
    /// service, the per-user config directory otherwise).
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Print a one-shot hardware discovery and telemetry JSON snapshot and exit.
    #[arg(long)]
    print_telemetry: bool,

    /// Run as the system service (Windows SCM / systemd): system data dir, no in-process session.
    #[arg(long)]
    service: bool,

    /// Run in the foreground of a user session (default). Session features (audio, notifications,
    /// foreground app) run in-process until an agent connects.
    #[arg(long)]
    foreground: bool,

    /// Never run session features in-process; always require the agent.
    #[arg(long)]
    no_in_process_session: bool,

    /// Windows, elevated: install (or repair) and start the service.
    #[arg(long)]
    install_service: bool,

    /// Windows, elevated: stop, replace binary and bridge, restart.
    #[arg(long)]
    upgrade_service: bool,

    /// Windows, elevated: remove the service, firewall rules and program files.
    #[arg(long)]
    uninstall_service: bool,

    /// With --uninstall-service: also delete the data directory (config, pairings, certificates).
    #[arg(long)]
    purge_data: bool,

    /// Generate a new IPC token. Running daemons pick it up within seconds and close their IPC
    /// sockets; the desktop app and the agent re-read the token file.
    #[arg(long)]
    rotate_ipc_token: bool,
}

/// Logs to stderr, or to `ctrlmated.log` in the data dir when running as a service (no console).
fn init_logging(cli: &Cli) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if cli.service {
        let dd = datadir::DataDir::resolve(cli.data_dir.clone(), true);
        if dd.prepare().is_ok() {
            let path = dd.file("ctrlmated.log");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
                let _ = std::fs::rename(&path, dd.file("ctrlmated.log.old"));
            }
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(file))
                    .init();
                return;
            }
        }
    }
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn wait_for_termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli);

    if cli.rotate_ipc_token {
        let dd = datadir::DataDir::resolve_for_cli(cli.data_dir.clone());
        ipc_token::rotate(&dd)?;
        println!(
            "IPC token rotated: {}\nRunning daemons switch within a few seconds; restart agents that do not re-read the file.",
            dd.file(ipc_token::TOKEN_FILE).display()
        );
        return Ok(());
    }
    if cli.install_service {
        return win_service::install_service(cli.ha_port, cli.ipc_port, cli.data_dir);
    }
    if cli.upgrade_service {
        return win_service::upgrade_service(cli.ha_port, cli.ipc_port, cli.data_dir);
    }
    if cli.uninstall_service {
        return win_service::uninstall_service(cli.purge_data);
    }
    if cli.service && cfg!(windows) {
        return win_service::run_as_windows_service(cli.ha_port, cli.ipc_port, cli.data_dir);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        if cli.print_telemetry {
            return print_telemetry(&cli).await;
        }
        let (stop_tx, stop_rx) = watch::channel(false);
        tokio::spawn(async move {
            wait_for_termination().await;
            let _ = stop_tx.send(true);
        });
        runtime::run_daemon(
            RunOptions {
                ha_port: cli.ha_port,
                ipc_port: cli.ipc_port,
                data_dir: cli.data_dir,
                service: cli.service,
                allow_in_process: !cli.no_in_process_session,
            },
            Hooks::default(),
            stop_rx,
        )
        .await
    })
}

async fn print_telemetry(cli: &Cli) -> Result<()> {
    let dd = datadir::DataDir::resolve(cli.data_dir.clone(), false);
    let cfg = config::DaemonConfig::load_or_create(&dd)?;
    let (id, mode, scripts) = (
        cfg.device_id.clone(),
        cfg.security_mode,
        cfg.scripts.clone(),
    );
    let out = tokio::task::spawn_blocking(move || {
        let mut collector = HardwareCollector::new(id, env!("CARGO_PKG_VERSION").to_string(), mode);
        let manifest = collector.build_manifest(&scripts);
        let _ = collector.poll_states();
        std::thread::sleep(Duration::from_millis(250));
        let (states, _) = collector.poll_states();
        serde_json::json!({
            "manifest": manifest,
            "states": states,
            "hid_devices": collector.hid_devices(),
            "rgb_devices": collector.rgb_device_statuses(),
            "conflicts": collector.detect_conflicts(),
        })
    })
    .await?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
