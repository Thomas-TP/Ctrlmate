//! `ctrlmate-agent`: headless user-session helper (no console window on Windows).
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use clap::Parser;
use ctrlmate_agent::client::ClientConfig;
use ctrlmate_agent::instance::InstanceLock;
use ctrlmate_agent::{autostart, logging, token};
use ctrlmate_hal::session::SessionRuntime;
use ctrlmate_protocol::DEFAULT_IPC_PORT;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "ctrlmate-agent",
    version,
    about = "Ctrlmate session agent: lock, display, notifications, audio, foreground app, scripts"
)]
struct Cli {
    /// Install autostart at login (HKCU Run on Windows, XDG autostart on Linux) and exit.
    #[arg(long)]
    install_autostart: bool,

    /// Remove autostart and exit.
    #[arg(long)]
    uninstall_autostart: bool,

    /// Print one JSON sample of the session state and exit (debugging; no daemon needed).
    #[arg(long)]
    once: bool,

    /// Loopback port of the daemon IPC listener (env CTRLMATE_IPC_PORT).
    #[arg(long)]
    ipc_port: Option<u16>,

    /// Full `host:port` of the daemon IPC listener; the host must be a loopback address.
    #[arg(long)]
    endpoint: Option<String>,

    /// Log level when CTRLMATE_LOG is not set.
    #[arg(long, default_value = "info")]
    log_level: String,
}

/// With the Windows GUI subsystem there is no console; attach to the parent's so that `--once`,
/// `--help` and the autostart commands can print when started from a terminal.
#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no preconditions; failure (no parent console, e.g. autostart) is expected and ignored.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

/// Only loopback endpoints are accepted: the token travels over plain HTTP.
fn validate_endpoint(ep: &str) -> Result<String> {
    let (host, port) = ep.rsplit_once(':').context("endpoint must be host:port")?;
    port.parse::<u16>().context("invalid port")?;
    let host_trim = host.trim_matches(['[', ']']);
    let loopback = host_trim.eq_ignore_ascii_case("localhost")
        || host_trim
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !loopback {
        bail!("refusing non-loopback endpoint '{ep}'");
    }
    Ok(ep.to_string())
}

fn endpoint(cli: &Cli) -> Result<String> {
    if let Some(e) = &cli.endpoint {
        return validate_endpoint(e);
    }
    let port = cli
        .ipc_port
        .or_else(|| {
            std::env::var("CTRLMATE_IPC_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
        })
        .unwrap_or(DEFAULT_IPC_PORT);
    Ok(format!("127.0.0.1:{port}"))
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

fn real_main() -> Result<ExitCode> {
    attach_parent_console();
    let cli = Cli::parse();

    if cli.install_autostart {
        println!("autostart installed: {}", autostart::install()?);
        return Ok(ExitCode::SUCCESS);
    }
    if cli.uninstall_autostart {
        let removed = autostart::uninstall()?;
        println!(
            "autostart {}",
            if removed {
                "removed"
            } else {
                "was not installed"
            }
        );
        return Ok(ExitCode::SUCCESS);
    }
    if cli.once {
        let state = SessionRuntime::new().sample();
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(ExitCode::SUCCESS);
    }

    logging::init(&cli.log_level);
    let endpoint = endpoint(&cli)?;
    let Some(_lock) = InstanceLock::acquire("CtrlmateAgent")? else {
        info!("Another ctrlmate-agent is already running in this session; exiting");
        return Ok(ExitCode::SUCCESS);
    };
    info!(
        "ctrlmate-agent {} starting (endpoint {endpoint})",
        ctrlmate_agent::AGENT_VERSION
    );
    if token::find_token().is_none() {
        warn!(
            "No IPC token found yet; will keep looking (env CTRLMATE_IPC_TOKEN, user dir, system dir)"
        );
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    rt.block_on(async move {
        let (stop_tx, stop_rx) = watch::channel(false);
        tokio::spawn(async move {
            wait_for_termination().await;
            let _ = stop_tx.send(true);
        });
        let cfg = ClientConfig::new(endpoint, Arc::new(|| token::find_token().map(|(t, _)| t)));
        ctrlmate_agent::run(cfg, Arc::new(SessionRuntime::new()), stop_rx).await;
    });
    info!("ctrlmate-agent stopped");
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match real_main() {
        Ok(c) => c,
        Err(e) => {
            error!("{e:#}");
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_endpoints_are_accepted() {
        for ok in [
            "127.0.0.1:9488",
            "localhost:9488",
            "[::1]:9488",
            "127.0.0.2:1",
        ] {
            assert!(validate_endpoint(ok).is_ok(), "{ok}");
        }
        for bad in [
            "192.168.1.5:9488",
            "example.com:9488",
            "0.0.0.0:9488",
            "127.0.0.1",
            "127.0.0.1:x",
        ] {
            assert!(validate_endpoint(bad).is_err(), "{bad}");
        }
    }
}
