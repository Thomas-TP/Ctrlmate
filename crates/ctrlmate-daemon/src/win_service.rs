//! Windows service integration for `ctrlmated` (architecture contract section 8).
//!
//! * `--service`            run inside the Service Control Manager (StartPending, Running,
//!   StopPending, Stopped reported properly; graceful stop restores the hardware).
//! * `--install-service`    elevated: copy to `%ProgramFiles%\Ctrlmate`, register the service
//!   (LocalSystem, auto start, recovery actions), firewall rules (private + domain, local subnet
//!   only: TCP 9487 + UDP 5353) and the data directory ACLs. Also repairs an existing install.
//! * `--upgrade-service`    stop, replace binary and bridge, reconfigure, start.
//! * `--uninstall-service`  stop, delete, remove firewall rules and program files
//!   (`--purge-data` additionally removes `%ProgramData%\Ctrlmate`).

// The constants and pure helpers below are unit-tested on every platform but only used by the Windows
// service code, so the Linux build would otherwise fail `-D warnings` on dead code.
#![cfg_attr(not(windows), allow(dead_code))]

use anyhow::Result;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const SERVICE_NAME: &str = "Ctrlmate";
pub const SERVICE_DISPLAY_NAME: &str = "Ctrlmate Hardware and Home Assistant Daemon";
pub const SERVICE_DESCRIPTION: &str =
    "Ctrlmate telemetry, lighting, cooling and Home Assistant bridge (TLS 1.3, LAN only).";
pub const FIREWALL_RULE_TLS: &str = "Ctrlmate-HA-TLS";
pub const FIREWALL_RULE_MDNS: &str = "Ctrlmate-mDNS";
pub const MDNS_PORT: u16 = 5353;

/// Command line the SCM uses to start the service. `--data-dir` is only passed when the user
/// explicitly chose a custom location: by default the service uses `%ProgramData%\Ctrlmate`.
pub fn launch_args(ha_port: u16, ipc_port: u16, data_dir: Option<&Path>) -> Vec<OsString> {
    let mut a = vec![
        OsString::from("--service"),
        OsString::from("--ha-port"),
        OsString::from(ha_port.to_string()),
        OsString::from("--ipc-port"),
        OsString::from(ipc_port.to_string()),
    ];
    if let Some(d) = data_dir {
        a.push(OsString::from("--data-dir"));
        a.push(d.as_os_str().to_os_string());
    }
    a
}

/// Files copied next to the installed daemon: the daemon itself plus the optional deep-hardware
/// bridge (and its .NET dependencies, which only exist beside it when the bridge is shipped).
pub fn payload_files(src_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(src_dir) else {
        return vec![];
    };
    let names: Vec<(String, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_ascii_lowercase(),
                e.path(),
            )
        })
        .collect();
    let has_bridge = names.iter().any(|(n, _)| n == "ctrlmate-lhm-bridge.exe");
    names
        .into_iter()
        .filter(|(n, _)| {
            n == "ctrlmated.exe"
                || n.starts_with("ctrlmate-lhm-bridge")
                || (has_bridge && (n.ends_with(".dll") || n.ends_with(".json")))
        })
        .map(|(_, p)| p)
        .collect()
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use crate::datadir::DataDir;
    use crate::runtime::{Hooks, RunOptions};
    use anyhow::{Context, anyhow, bail};
    use ctrlmate_protocol::DataDirKind;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::{Duration, Instant};
    use tokio::sync::watch;
    use tracing::{error, info, warn};
    use windows_service::service::{
        Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceControl,
        ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceFailureActions,
        ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const START_WAIT_HINT: Duration = Duration::from_secs(60);
    const STOP_WAIT_HINT: Duration = Duration::from_secs(30);

    struct Launch {
        ha_port: u16,
        ipc_port: u16,
        data_dir: Option<PathBuf>,
    }

    static LAUNCH: OnceLock<Launch> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn run_as_windows_service(
        ha_port: u16,
        ipc_port: u16,
        data_dir: Option<PathBuf>,
    ) -> Result<()> {
        let _ = LAUNCH.set(Launch {
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

    fn status(
        state: ServiceState,
        accepted: ServiceControlAccept,
        checkpoint: u32,
        hint: Duration,
        exit: u32,
    ) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accepted,
            exit_code: ServiceExitCode::Win32(exit),
            checkpoint,
            wait_hint: hint,
            process_id: None,
        }
    }

    fn run_service_body() -> Result<()> {
        let (stop_tx, stop_rx) = watch::channel(false);
        let handle: ServiceStatusHandle =
            service_control_handler::register(SERVICE_NAME, move |ev| match ev {
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Preshutdown | ServiceControl::Shutdown => {
                    let _ = stop_tx.send(true);
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            })?;

        handle.set_service_status(status(
            ServiceState::StartPending,
            ServiceControlAccept::empty(),
            1,
            START_WAIT_HINT,
            0,
        ))?;

        // Keep the SCM informed while the hardware probe (HID, NVML, WMI) runs.
        let ready = Arc::new(AtomicBool::new(false));
        let checkpoint = Arc::new(AtomicU32::new(1));
        {
            let (ready, checkpoint) = (Arc::clone(&ready), Arc::clone(&checkpoint));
            std::thread::spawn(move || {
                while !ready.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_secs(2));
                    if ready.load(Ordering::Acquire) {
                        break;
                    }
                    let cp = checkpoint.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = handle.set_service_status(status(
                        ServiceState::StartPending,
                        ServiceControlAccept::empty(),
                        cp,
                        START_WAIT_HINT,
                        0,
                    ));
                }
            });
        }

        let launch = LAUNCH.get();
        let opts = RunOptions {
            ha_port: launch.map_or(ctrlmate_protocol::DEFAULT_HA_PORT, |l| l.ha_port),
            ipc_port: launch.map_or(ctrlmate_protocol::DEFAULT_IPC_PORT, |l| l.ipc_port),
            data_dir: launch.and_then(|l| l.data_dir.clone()),
            service: true,
            allow_in_process: false,
        };
        let hooks = Hooks {
            on_ready: Some(Box::new({
                let ready = Arc::clone(&ready);
                move || {
                    ready.store(true, Ordering::Release);
                    let _ = handle.set_service_status(status(
                        ServiceState::Running,
                        ServiceControlAccept::STOP | ServiceControlAccept::PRESHUTDOWN,
                        0,
                        Duration::default(),
                        0,
                    ));
                }
            })),
            on_stopping: Some(Box::new(move || {
                let _ = handle.set_service_status(status(
                    ServiceState::StopPending,
                    ServiceControlAccept::empty(),
                    1,
                    STOP_WAIT_HINT,
                    0,
                ));
            })),
        };

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let result = rt.block_on(crate::runtime::run_daemon(opts, hooks, stop_rx));
        ready.store(true, Ordering::Release);
        if let Err(e) = &result {
            error!("Daemon exited with error: {e:#}");
        }
        handle.set_service_status(status(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            0,
            Duration::default(),
            u32::from(result.is_err()),
        ))?;
        result
    }

    // --- install / upgrade / uninstall --------------------------------------------------------

    fn run_tool(program: &str, args: &[&str]) -> Result<std::process::Output> {
        Command::new(program)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .with_context(|| format!("failed to run {program}"))
    }

    /// Elevated = high mandatory integrity level (S-1-16-12288), locale independent.
    fn require_elevation() -> Result<()> {
        let out = run_tool("whoami", &["/groups", "/fo", "csv", "/nh"])?;
        if String::from_utf8_lossy(&out.stdout).contains("S-1-16-12288") {
            Ok(())
        } else {
            bail!("this command must run from an elevated (Administrator) prompt")
        }
    }

    fn program_dir() -> PathBuf {
        std::env::var_os("ProgramFiles")
            .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
            .join("Ctrlmate")
    }

    fn installed_exe() -> PathBuf {
        program_dir().join("ctrlmated.exe")
    }

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access)
            .context("cannot open the Service Control Manager (elevation required)")
    }

    fn open(m: &ServiceManager, access: ServiceAccess) -> Option<Service> {
        m.open_service(SERVICE_NAME, access).ok()
    }

    const FULL: ServiceAccess = ServiceAccess::QUERY_STATUS
        .union(ServiceAccess::START)
        .union(ServiceAccess::STOP)
        .union(ServiceAccess::CHANGE_CONFIG)
        .union(ServiceAccess::DELETE);

    fn stop_and_wait(service: &Service) -> Result<()> {
        if service.query_status()?.current_state == ServiceState::Stopped {
            return Ok(());
        }
        let _ = service.stop();
        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline {
            if service.query_status()?.current_state == ServiceState::Stopped {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        bail!("service did not stop within 40 s")
    }

    fn copy_payload(dst: &Path) -> Result<()> {
        let exe = std::env::current_exe()?;
        let src_dir = exe.parent().context("daemon has no parent directory")?;
        std::fs::create_dir_all(dst)?;
        if same_dir(src_dir, dst) {
            return Ok(());
        }
        let mut files = payload_files(src_dir);
        if !files.iter().any(|f| {
            f.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("ctrlmated.exe"))
        }) {
            // Running under another file name (e.g. cargo's target dir): install ourselves.
            files.push(exe.clone());
        }
        for f in files {
            let name = if f == exe {
                OsString::from("ctrlmated.exe")
            } else {
                f.file_name().unwrap_or_default().to_os_string()
            };
            std::fs::copy(&f, dst.join(&name))
                .with_context(|| format!("failed to copy {}", f.display()))?;
        }
        Ok(())
    }

    fn same_dir(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
    }

    /// Program files are writable by administrators only; everyone may read and execute.
    fn protect_program_dir(dir: &Path) -> Result<()> {
        let out = Command::new("icacls")
            .arg(dir)
            .args([
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F",
                "*S-1-5-32-544:(OI)(CI)F",
                "*S-1-5-32-545:(OI)(CI)RX",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if !out.status.success() {
            bail!(
                "icacls failed on {}: {}",
                dir.display(),
                String::from_utf8_lossy(&out.stdout)
            );
        }
        Ok(())
    }

    fn delete_firewall_rules() {
        for name in [FIREWALL_RULE_TLS, FIREWALL_RULE_MDNS] {
            let _ = run_tool(
                "netsh",
                &[
                    "advfirewall",
                    "firewall",
                    "delete",
                    "rule",
                    &format!("name={name}"),
                ],
            );
        }
    }

    /// Inbound rules for the private and domain profiles, restricted to the local subnet.
    fn configure_firewall(exe: &Path, ha_port: u16) -> Result<()> {
        delete_firewall_rules();
        let exe_arg = format!("program={}", exe.display());
        let rules: [(String, &str, String); 2] = [
            (
                format!("name={FIREWALL_RULE_TLS}"),
                "TCP",
                format!("localport={ha_port}"),
            ),
            (
                format!("name={FIREWALL_RULE_MDNS}"),
                "UDP",
                format!("localport={MDNS_PORT}"),
            ),
        ];
        for (name, proto, port) in rules {
            let proto_arg = format!("protocol={proto}");
            let out = run_tool(
                "netsh",
                &[
                    "advfirewall",
                    "firewall",
                    "add",
                    "rule",
                    &name,
                    "dir=in",
                    "action=allow",
                    &proto_arg,
                    &port,
                    "profile=private,domain",
                    "remoteip=localsubnet",
                    &exe_arg,
                ],
            )?;
            if !out.status.success() {
                bail!(
                    "netsh failed for {name}: {}",
                    String::from_utf8_lossy(&out.stdout)
                );
            }
        }
        Ok(())
    }

    fn set_recovery(service: &Service) -> Result<()> {
        let restart = |secs| ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: Duration::from_secs(secs),
        };
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![restart(5), restart(30), restart(60)]),
        })?;
        service.set_failure_actions_on_non_crash_failures(true)?;
        Ok(())
    }

    fn service_info(
        exe: PathBuf,
        ha_port: u16,
        ipc_port: u16,
        data_dir: Option<&Path>,
    ) -> ServiceInfo {
        ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe,
            launch_arguments: launch_args(ha_port, ipc_port, data_dir),
            dependencies: vec![],
            // LocalSystem: NVMe SMART, Windows Update Agent, HID, optional PawnIO.
            account_name: None,
            account_password: None,
        }
    }

    fn prepare_data_dir(custom: Option<&Path>) -> Result<DataDir> {
        let dd = DataDir::resolve(custom.map(Path::to_path_buf), true);
        dd.prepare()?;
        // Create the IPC token now so the desktop app can connect right away.
        crate::ipc_token::load_or_create(&dd)?;
        Ok(dd)
    }

    pub fn install_service(ha_port: u16, ipc_port: u16, data_dir: Option<PathBuf>) -> Result<()> {
        require_elevation()?;
        let m = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let existing = open(&m, FULL);
        if let Some(s) = &existing {
            println!("Existing service found: stopping it for the upgrade...");
            stop_and_wait(s)?;
        }

        let dir = program_dir();
        copy_payload(&dir)?;
        protect_program_dir(&dir)?;
        let exe = installed_exe();
        let dd = prepare_data_dir(data_dir.as_deref())?;
        let info = service_info(exe.clone(), ha_port, ipc_port, data_dir.as_deref());

        let service = match existing {
            Some(s) => {
                s.change_config(&info)?;
                s
            }
            None => m.create_service(&info, FULL)?,
        };
        service.set_description(SERVICE_DESCRIPTION)?;
        set_recovery(&service)?;
        configure_firewall(&exe, ha_port)?;
        service.start(&[] as &[&str])?;
        println!(
            "Ctrlmate service installed and started.\n  program: {}\n  data:    {} ({:?})\n  LAN:     TCP {ha_port} + UDP {MDNS_PORT}, private + domain profiles, local subnet only",
            dir.display(),
            dd.path.display(),
            dd.kind,
        );
        info!("Windows Service '{SERVICE_NAME}' installed");
        Ok(())
    }

    pub fn upgrade_service(ha_port: u16, ipc_port: u16, data_dir: Option<PathBuf>) -> Result<()> {
        require_elevation()?;
        let m = manager(ServiceManagerAccess::CONNECT)?;
        let service = open(&m, FULL)
            .context("the Ctrlmate service is not installed (use --install-service)")?;
        stop_and_wait(&service)?;
        let dir = program_dir();
        copy_payload(&dir)?;
        protect_program_dir(&dir)?;
        let exe = installed_exe();
        prepare_data_dir(data_dir.as_deref())?;
        service.change_config(&service_info(
            exe.clone(),
            ha_port,
            ipc_port,
            data_dir.as_deref(),
        ))?;
        set_recovery(&service)?;
        configure_firewall(&exe, ha_port)?;
        service.start(&[] as &[&str])?;
        println!("Ctrlmate service upgraded and restarted.");
        Ok(())
    }

    pub fn uninstall_service(purge_data: bool) -> Result<()> {
        require_elevation()?;
        let m = manager(ServiceManagerAccess::CONNECT)?;
        if let Some(service) = open(&m, FULL) {
            stop_and_wait(&service)?;
            service.delete()?;
        } else {
            println!("Service not registered; cleaning up leftovers.");
        }
        delete_firewall_rules();
        let dir = program_dir();
        if let Err(e) = std::fs::remove_dir_all(&dir)
            && dir.exists()
        {
            warn!(
                "Could not remove {} ({e}); delete it manually",
                dir.display()
            );
        }
        if purge_data {
            let dd = DataDir::resolve(None, true);
            if dd.kind == DataDirKind::System {
                let _ = std::fs::remove_dir_all(&dd.path);
            }
        }
        println!("Ctrlmate service uninstalled.");
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::*;
    use anyhow::bail;

    pub fn run_as_windows_service(_: u16, _: u16, _: Option<PathBuf>) -> Result<()> {
        bail!("--service via the Windows SCM is only available on Windows")
    }
    pub fn install_service(_: u16, _: u16, _: Option<PathBuf>) -> Result<()> {
        bail!(
            "--install-service is only supported on Windows (use packaging/linux/ctrlmated.service on Linux)"
        )
    }
    pub fn upgrade_service(_: u16, _: u16, _: Option<PathBuf>) -> Result<()> {
        bail!("--upgrade-service is only supported on Windows")
    }
    pub fn uninstall_service(_: bool) -> Result<()> {
        bail!("--uninstall-service is only supported on Windows")
    }
}

pub use imp::{install_service, run_as_windows_service, uninstall_service, upgrade_service};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_pass_ports_and_only_an_explicit_data_dir() {
        let a = launch_args(9487, 9488, None);
        let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert_eq!(s, ["--service", "--ha-port", "9487", "--ipc-port", "9488"]);
        let custom = launch_args(1, 2, Some(Path::new(r"D:\data")));
        assert_eq!(custom.len(), 7);
        assert_eq!(custom[6], OsString::from(r"D:\data"));
    }

    #[test]
    fn payload_selection_takes_daemon_and_bridge_only() {
        let tmp = tempfile::tempdir().unwrap();
        for f in [
            "ctrlmated.exe",
            "ctrlmate-lhm-bridge.exe",
            "ctrlmate-lhm-bridge.runtimeconfig.json",
            "LibreHardwareMonitorLib.dll",
            "other-app.exe",
            "readme.txt",
        ] {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        let mut names: Vec<String> = payload_files(tmp.path())
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_ascii_lowercase()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "ctrlmate-lhm-bridge.exe",
                "ctrlmate-lhm-bridge.runtimeconfig.json",
                "ctrlmated.exe",
                "librehardwaremonitorlib.dll",
            ]
        );
    }

    #[test]
    fn dlls_without_a_bridge_are_not_copied() {
        let tmp = tempfile::tempdir().unwrap();
        for f in ["ctrlmated.exe", "webview2loader.dll", "app.json"] {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        let names: Vec<_> = payload_files(tmp.path());
        assert_eq!(names.len(), 1);
    }
}
