//! Native Windows Service (SCM) integration for `ctrlmated`.
//!
//! Supports:
//! - `ctrlmated --service` (run inside Windows Service Control Manager)
//! - `ctrlmated --install-service` (installs auto-start `Ctrlmate` service + LAN firewall rule)
//! - `ctrlmated --uninstall-service` (stops & removes the `Ctrlmate` service)

use anyhow::Result;
use std::path::PathBuf;

pub const SERVICE_NAME: &str = "Ctrlmate";
pub const SERVICE_DISPLAY_NAME: &str = "Ctrlmate Hardware & Home Assistant Daemon";

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use anyhow::anyhow;
    use std::ffi::OsString;
    use std::process::Command;
    use std::sync::OnceLock;
    use std::time::Duration;
    use tokio::sync::watch;
    use tracing::{error, info};
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    #[derive(Clone)]
    struct ServiceLaunchConfig {
        ha_port: u16,
        ipc_port: u16,
        data_dir: Option<PathBuf>,
    }

    static LAUNCH_CFG: OnceLock<ServiceLaunchConfig> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn run_as_windows_service(
        ha_port: u16,
        ipc_port: u16,
        data_dir: Option<PathBuf>,
    ) -> Result<()> {
        let _ = LAUNCH_CFG.set(ServiceLaunchConfig {
            ha_port,
            ipc_port,
            data_dir,
        });
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .map_err(|e| anyhow!("Windows SCM dispatch failed: {e}"))
    }

    fn service_main(_args: Vec<OsString>) {
        if let Err(err) = run_service_body() {
            error!("Ctrlmate Windows service terminated with error: {err:#}");
        }
    }

    fn run_service_body() -> Result<()> {
        let (stop_tx, stop_rx) = watch::channel(false);

        let status_handle = service_control_handler::register(
            SERVICE_NAME,
            move |control_event| match control_event {
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Preshutdown | ServiceControl::Shutdown => {
                    let _ = stop_tx.send(true);
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            },
        )?;

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::PRESHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        let launch = LAUNCH_CFG.get().cloned().unwrap_or(ServiceLaunchConfig {
            ha_port: ctrlmate_protocol::DEFAULT_HA_PORT,
            ipc_port: ctrlmate_protocol::DEFAULT_IPC_PORT,
            data_dir: Some(PathBuf::from(r"C:\ProgramData\Ctrlmate")),
        });

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let result = rt.block_on(crate::run_daemon(
            launch.ha_port,
            launch.ipc_port,
            launch.data_dir,
            stop_rx,
        ));

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(if result.is_ok() { 0 } else { 1 }),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        result
    }

    pub fn install_windows_service(ha_port: u16, data_dir: Option<PathBuf>) -> Result<()> {
        let exe_path = std::env::current_exe()?;
        let effective_dir = data_dir.unwrap_or_else(|| PathBuf::from(r"C:\ProgramData\Ctrlmate"));
        let _ = std::fs::create_dir_all(&effective_dir);

        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )?;

        let service_info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe_path,
            launch_arguments: vec![
                OsString::from("--service"),
                OsString::from("--ha-port"),
                OsString::from(ha_port.to_string()),
                OsString::from("--data-dir"),
                effective_dir.into_os_string(),
            ],
            dependencies: vec![],
            account_name: None, // LocalSystem (allows NVMe SMART, WUA, PawnIO opt-in, Kraken HID)
            account_password: None,
        };

        let service = match manager.open_service(
            SERVICE_NAME,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START,
        ) {
            Ok(existing) => {
                existing.change_config(&service_info)?;
                existing
            }
            Err(_) => manager.create_service(
                &service_info,
                ServiceAccess::CHANGE_CONFIG | ServiceAccess::START,
            )?,
        };

        let _ = service.set_description(
            "Ctrlmate cross-platform telemetry, lighting, cooling & Home Assistant 2026.11 service.",
        );

        // Ensure LAN inbound rule exists for Home Assistant WSS/HTTPS port
        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                "name=Ctrlmate Home Assistant TLS",
                "dir=in",
                "action=allow",
                "protocol=TCP",
                &format!("localport={ha_port}"),
                "profile=private",
            ])
            .output();

        let _ = service.start(&[] as &[&str]);
        info!("Windows Service '{SERVICE_NAME}' installed and started (TCP {ha_port})");
        Ok(())
    }

    pub fn uninstall_windows_service() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
        )?;
        let _ = service.stop();
        service.delete()?;
        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                "name=Ctrlmate Home Assistant TLS",
            ])
            .output();
        info!("Windows Service '{SERVICE_NAME}' uninstalled");
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::*;
    use anyhow::bail;

    pub fn run_as_windows_service(
        _ha_port: u16,
        _ipc_port: u16,
        _data_dir: Option<PathBuf>,
    ) -> Result<()> {
        bail!("--service is only supported on Windows (use systemd ctrlmated.service on Linux)")
    }

    pub fn install_windows_service(_ha_port: u16, _data_dir: Option<PathBuf>) -> Result<()> {
        bail!("--install-service is only supported on Windows (use packaging/linux/ctrlmated.service on Linux)")
    }

    pub fn uninstall_windows_service() -> Result<()> {
        bail!("--uninstall-service is only supported on Windows")
    }
}

pub use imp::{install_windows_service, run_as_windows_service, uninstall_windows_service};
