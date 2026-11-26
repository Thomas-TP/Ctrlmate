//! Manager of the optional Windows deep-hardware bridge (`ctrlmate-lhm-bridge.exe`).
//!
//! * Never started in the default zero-kernel mode and never while an anti-cheat runs
//!   (the collector's anti-cheat gate decides; see [`crate::anticheat`]).
//! * Speaks the bridge's JSON line protocol (`hello`, `heartbeat`, `snapshot`, `ack`, `error`,
//!   `anti_cheat`, `refused`, `stopped`; see `crates/ctrlmate-lhm-bridge/README.md`).
//! * Liveness: [`LhmBridgeManager::check`] uses `try_wait` plus the heartbeat contract (no line for
//!   3 s, or `poll_age_ms` above 5 s, means dead / hung). A dead bridge clears its snapshot, so its
//!   sensors are never reported as fresh, and the cooling engine sees them go stale.
//! * Stop sequence: `reset_pwm` + `quit`, wait for `stopped{verified}` (PWM back on firmware control,
//!   confirmed by the bridge reading the mode back), wait for the exit, kill only as a last resort.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// A snapshot older than this is not reported (the bridge emits one per second).
pub const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(3);
/// No line at all for this long: process dead or blocked on stdout.
pub const SILENCE_TIMEOUT: Duration = Duration::from_secs(3);
/// Heartbeats continue but the hardware poll is older than this: poll hung.
pub const POLL_HUNG_MS: u64 = 5000;
/// Time a freshly spawned bridge gets to emit its first line (driver init can be slow).
pub const STARTUP_GRACE: Duration = Duration::from_secs(15);
const ACK_TIMEOUT: Duration = Duration::from_millis(2500);
const STOPPED_TIMEOUT: Duration = Duration::from_secs(6);
const EXIT_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NamedTemp {
    pub name: String,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CaseFanReading {
    pub id: String,
    pub name: String,
    pub rpm: Option<f64>,
}

/// One bridge `snapshot`. Missing sensors are `None`, never a default.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct DeepSensorSnapshot {
    /// Tctl on AMD, else package, else the first CPU temperature (chosen by the bridge).
    #[serde(default)]
    pub cpu_temp_c: Option<f64>,
    #[serde(default)]
    pub cpu_tctl_c: Option<f64>,
    #[serde(default)]
    pub cpu_package_c: Option<f64>,
    #[serde(default)]
    pub cpu_ccd_temps_c: Vec<NamedTemp>,
    #[serde(default)]
    pub cpu_power_w: Option<f64>,
    #[serde(default)]
    pub motherboard_name: Option<String>,
    #[serde(default)]
    pub motherboard_vrm_temp_c: Option<f64>,
    #[serde(default)]
    pub motherboard_chipset_temp_c: Option<f64>,
    #[serde(default)]
    pub ram_temp_c: Option<f64>,
    #[serde(default)]
    pub case_fans: Vec<CaseFanReading>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BridgeLine {
    Hello,
    Heartbeat {
        #[serde(default)]
        poll_age_ms: Option<u64>,
    },
    Snapshot(DeepSensorSnapshot),
    Ack {
        cmd: String,
        ok: bool,
        #[serde(default)]
        detail: Option<String>,
    },
    Error {
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        detail: Option<String>,
    },
    AntiCheat,
    Refused {
        #[serde(default)]
        reason: Option<String>,
    },
    Stopped {
        #[serde(default)]
        pwm_reset: bool,
        #[serde(default)]
        verified: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone)]
struct Ack {
    cmd: String,
    ok: bool,
    detail: Option<String>,
}

/// Result of the stop sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StopOutcome {
    /// There was a bridge to stop.
    pub was_running: bool,
    /// The bridge confirmed that every PWM header is back on firmware control.
    pub pwm_reset_verified: bool,
    /// The process had to be killed (no clean exit within the timeout).
    pub killed: bool,
}

/// Health of the bridge process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeHealth {
    NotStarted,
    Running,
    /// The process ended on its own (`3` refused, `4` anti-cheat appeared, `5` error, ...).
    Exited(Option<i32>),
    /// Silent or hung: the caller should [`LhmBridgeManager::stop`] it.
    Unresponsive,
}

#[derive(Default)]
struct SharedInner {
    snapshot: Option<(DeepSensorSnapshot, Instant)>,
    last_line: Option<Instant>,
    poll_age_ms: Option<u64>,
    acks: Vec<Ack>,
    stopped: Option<(bool, bool)>,
    refused: Option<String>,
    anti_cheat: bool,
}

#[derive(Default)]
struct Shared {
    inner: Mutex<SharedInner>,
    cv: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared {
    /// Parses and applies one stdout line. Unparseable lines are ignored (but count as liveness).
    fn handle_line(&self, line: &str, now: Instant) {
        let mut g = lock(&self.inner);
        g.last_line = Some(now);
        let Ok(msg) = serde_json::from_str::<BridgeLine>(line) else {
            return;
        };
        match msg {
            BridgeLine::Snapshot(s) => {
                g.snapshot = Some((s, now));
            }
            BridgeLine::Heartbeat { poll_age_ms } => g.poll_age_ms = poll_age_ms,
            BridgeLine::Ack { cmd, ok, detail } => {
                g.acks.push(Ack { cmd, ok, detail });
                if g.acks.len() > 32 {
                    g.acks.remove(0);
                }
            }
            BridgeLine::Stopped {
                pwm_reset,
                verified,
            } => g.stopped = Some((pwm_reset, verified)),
            BridgeLine::Refused { reason } => g.refused = reason.or(Some("unknown".into())),
            BridgeLine::AntiCheat => g.anti_cheat = true,
            BridgeLine::Error { error, detail } => {
                warn!("LHM bridge error: {error:?} {detail:?}");
            }
            BridgeLine::Hello | BridgeLine::Unknown => {}
        }
        drop(g);
        self.cv.notify_all();
    }

    fn clear_data(&self) {
        let mut g = lock(&self.inner);
        g.snapshot = None;
        g.poll_age_ms = None;
    }

    fn reset_for_start(&self) {
        *lock(&self.inner) = SharedInner::default();
    }

    fn fresh_snapshot(&self, now: Instant) -> Option<DeepSensorSnapshot> {
        lock(&self.inner)
            .snapshot
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) <= SNAPSHOT_MAX_AGE)
            .map(|(s, _)| s.clone())
    }

    /// Liveness of a bridge started at `started`, given that the process still exists.
    fn alive(&self, started: Instant, now: Instant) -> bool {
        let g = lock(&self.inner);
        match g.last_line {
            None => now.saturating_duration_since(started) <= STARTUP_GRACE,
            Some(at) => {
                now.saturating_duration_since(at) <= SILENCE_TIMEOUT
                    && g.poll_age_ms.is_none_or(|a| a <= POLL_HUNG_MS)
            }
        }
    }

    fn wait_ack(&self, cmd: &str, timeout: Duration) -> Option<Ack> {
        let deadline = Instant::now() + timeout;
        let mut g = lock(&self.inner);
        loop {
            if let Some(pos) = g.acks.iter().position(|a| a.cmd == cmd) {
                return Some(g.acks.remove(pos));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    fn wait_stopped(&self, timeout: Duration) -> Option<(bool, bool)> {
        let deadline = Instant::now() + timeout;
        let mut g = lock(&self.inner);
        loop {
            if let Some(s) = g.stopped {
                return Some(s);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    started: Instant,
}

/// Owner of the bridge process. All methods take `&self`: the cooling thread, the supervisor and
/// the collector share one `Arc<LhmBridgeManager>`.
pub struct LhmBridgeManager {
    proc: Mutex<Option<Proc>>,
    shared: Arc<Shared>,
    chassis_ids: Vec<String>,
}

impl Default for LhmBridgeManager {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl LhmBridgeManager {
    /// `chassis_ids` are LibreHardwareMonitor control identifiers passed as `--chassis-id`.
    pub fn new(chassis_ids: Vec<String>) -> Self {
        Self {
            proc: Mutex::new(None),
            shared: Arc::new(Shared::default()),
            chassis_ids,
        }
    }

    pub fn is_running(&self) -> bool {
        lock(&self.proc).is_some()
    }

    /// Fresh snapshot, or `None` when the bridge is not running or its data is stale.
    pub fn latest(&self) -> Option<DeepSensorSnapshot> {
        if !self.is_running() {
            return None;
        }
        self.shared.fresh_snapshot(Instant::now())
    }

    /// Why the bridge refused to start / stopped by itself (`anti_cheat_active`, `no_driver_flag`, ...).
    pub fn refusal(&self) -> Option<String> {
        let g = lock(&self.shared.inner);
        g.refused
            .clone()
            .or_else(|| g.anti_cheat.then(|| "anti_cheat_active".to_string()))
    }

    /// Process + heartbeat liveness. A bridge that exited or went silent loses its snapshot here.
    pub fn check(&self, now: Instant) -> BridgeHealth {
        let mut guard = lock(&self.proc);
        let Some(p) = guard.as_mut() else {
            return BridgeHealth::NotStarted;
        };
        match p.child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code();
                *guard = None;
                drop(guard);
                self.shared.clear_data();
                warn!("LHM bridge exited on its own (code {code:?})");
                BridgeHealth::Exited(code)
            }
            Ok(None) => {
                if self.shared.alive(p.started, now) {
                    BridgeHealth::Running
                } else {
                    drop(guard);
                    self.shared.clear_data();
                    error!("LHM bridge is silent or its poll is hung; its sensors are now lost");
                    BridgeHealth::Unresponsive
                }
            }
            Err(e) => {
                warn!("LHM bridge try_wait failed: {e}");
                BridgeHealth::Unresponsive
            }
        }
    }

    /// Starts the helper if it exists. Returns `false` (and logs) when it cannot start.
    pub fn start(&self) -> bool {
        if self.is_running() {
            return true;
        }
        if !cfg!(target_os = "windows") {
            return false;
        }
        let Some(exe) = find_bridge_executable() else {
            warn!("Deep hardware bridge ctrlmate-lhm-bridge.exe not found next to the daemon");
            return false;
        };
        let mut cmd = Command::new(&exe);
        for id in &self.chassis_ids {
            cmd.arg("--chassis-id").arg(id);
        }
        self.spawn(cmd, &exe)
    }

    fn spawn(&self, mut cmd: Command, exe: &std::path::Path) -> bool {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        self.shared.reset_for_start();
        match cmd.spawn() {
            Ok(mut child) => {
                let stdin = child.stdin.take();
                if let Some(stdout) = child.stdout.take() {
                    let shared = Arc::clone(&self.shared);
                    let _ = std::thread::Builder::new()
                        .name("ctrlmate-bridge-rx".into())
                        .spawn(move || {
                            for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                                shared.handle_line(&line, Instant::now());
                            }
                        });
                }
                info!("Started deep hardware bridge: {}", exe.display());
                *lock(&self.proc) = Some(Proc {
                    child,
                    stdin,
                    started: Instant::now(),
                });
                true
            }
            Err(err) => {
                error!("Failed to spawn the deep hardware bridge: {err}");
                false
            }
        }
    }

    fn send(&self, line: &str) -> Result<()> {
        let mut guard = lock(&self.proc);
        let p = guard.as_mut().context("bridge not running")?;
        let stdin = p.stdin.as_mut().context("bridge stdin closed")?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .context("bridge stdin write failed")
    }

    /// Chassis fans: `Some(percent)` = software duty, `None` = back to firmware control. Waits for the
    /// bridge's `ack`; an error means the write did not happen (the caller retries). Returns the number
    /// of chassis controls driven (0 is legal: the board has none configured).
    pub fn set_case_fans(&self, percent: Option<u8>) -> Result<usize> {
        let (cmd, line) = match percent {
            Some(p) => (
                "set_all_pwm",
                format!(r#"{{"cmd":"set_all_pwm","percent":{}}}"#, p.clamp(25, 100)),
            ),
            None => ("reset_pwm", r#"{"cmd":"reset_pwm"}"#.to_string()),
        };
        {
            // Drop stale acks of the same command so we wait for ours.
            lock(&self.shared.inner).acks.retain(|a| a.cmd != cmd);
        }
        self.send(&line)?;
        match self.shared.wait_ack(cmd, ACK_TIMEOUT) {
            Some(Ack {
                ok: true, detail, ..
            }) => Ok(detail
                .as_deref()
                .and_then(|d| d.strip_prefix("chassis_controls="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0)),
            Some(Ack {
                ok: false, detail, ..
            }) => {
                bail!("bridge refused {cmd}: {}", detail.unwrap_or_default())
            }
            None => bail!("bridge did not acknowledge {cmd}"),
        }
    }

    /// Full stop sequence; see the module documentation.
    pub fn stop(&self) -> StopOutcome {
        let Some(mut p) = lock(&self.proc).take() else {
            self.shared.clear_data();
            return StopOutcome::default();
        };
        let mut out = StopOutcome {
            was_running: true,
            ..StopOutcome::default()
        };
        lock(&self.shared.inner).stopped = None;
        if let Some(mut stdin) = p.stdin.take() {
            let _ = stdin.write_all(b"{\"cmd\":\"reset_pwm\"}\n{\"cmd\":\"quit\"}\n");
            let _ = stdin.flush();
            // Dropping stdin also tells the bridge the daemon is gone.
        }
        match self.shared.wait_stopped(STOPPED_TIMEOUT) {
            Some((reset, verified)) => {
                out.pwm_reset_verified = reset && verified;
                if !out.pwm_reset_verified {
                    error!(
                        "LHM bridge stopped but could not verify the PWM reset (reset={reset}, verified={verified})"
                    );
                }
            }
            None => error!("LHM bridge did not confirm the PWM reset"),
        }
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match p.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    warn!("LHM bridge did not exit in time: killing it");
                    let _ = p.child.kill();
                    let _ = p.child.wait();
                    out.killed = true;
                    break;
                }
            }
        }
        self.shared.clear_data();
        info!("Stopped the deep hardware bridge ({out:?})");
        out
    }
}

impl Drop for LhmBridgeManager {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Absolute candidates only: next to the daemon, then the repository build output above the
/// executable (development). The current working directory is never consulted.
fn find_bridge_executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![
        dir.join("ctrlmate-lhm-bridge.exe"),
        dir.join("bridge").join("ctrlmate-lhm-bridge.exe"),
    ];
    for root in dir.ancestors().skip(1).take(4) {
        let base = root.join("crates").join("ctrlmate-lhm-bridge").join("bin");
        candidates.push(base.join("Release/net8.0-windows/win-x64/ctrlmate-lhm-bridge.exe"));
        candidates.push(base.join("Release/net8.0-windows/ctrlmate-lhm-bridge.exe"));
    }
    candidates
        .into_iter()
        .find(|p| p.is_absolute() && p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAP: &str = r#"{"type":"snapshot","cpu_temp_c":71.5,"cpu_tctl_c":71.5,"cpu_package_c":null,"cpu_ccd_temps_c":[{"name":"CCD1","value":65.0},{"name":"CCD2","value":null}],"cpu_power_w":88.0,"motherboard_name":"X670","motherboard_vrm_temp_c":null,"motherboard_chipset_temp_c":50.0,"ram_temp_c":null,"case_fans":[{"id":"/lpc/it8688e/0/fan/1","name":"Fan #1","rpm":0},{"id":"/lpc/it8688e/0/fan/2","name":"Fan #2","rpm":1200.5}],"controls":[]}"#;

    #[test]
    fn parses_snapshot_lines_with_nulls() {
        let sh = Shared::default();
        let t = Instant::now();
        sh.handle_line(SNAP, t);
        let s = sh.fresh_snapshot(t).expect("snapshot");
        assert_eq!(s.cpu_temp_c, Some(71.5));
        assert_eq!(s.cpu_package_c, None);
        assert_eq!(s.cpu_ccd_temps_c.len(), 2);
        assert_eq!(s.cpu_ccd_temps_c[1].value, None);
        assert_eq!(s.motherboard_vrm_temp_c, None);
        assert_eq!(s.case_fans[0].rpm, Some(0.0));
        assert_eq!(s.case_fans.len(), 2);
    }

    #[test]
    fn stale_snapshots_are_not_reported() {
        let sh = Shared::default();
        let t = Instant::now();
        sh.handle_line(SNAP, t);
        assert!(sh.fresh_snapshot(t + Duration::from_secs(3)).is_some());
        assert!(sh.fresh_snapshot(t + Duration::from_secs(4)).is_none());
    }

    #[test]
    fn liveness_follows_the_heartbeat_contract() {
        let sh = Shared::default();
        let t0 = Instant::now();
        // Nothing yet: startup grace.
        assert!(sh.alive(t0, t0 + Duration::from_secs(10)));
        assert!(!sh.alive(t0, t0 + Duration::from_secs(16)));
        sh.handle_line(
            r#"{"type":"heartbeat","seq":1,"ts_ms":1,"poll_age_ms":120,"polls":3}"#,
            t0,
        );
        assert!(sh.alive(t0, t0 + Duration::from_secs(2)));
        // Silent for > 3 s
        assert!(!sh.alive(t0, t0 + Duration::from_secs(4)));
        // Heartbeats continue but the poll is hung
        sh.handle_line(
            r#"{"type":"heartbeat","seq":9,"ts_ms":9,"poll_age_ms":7000,"polls":3}"#,
            t0 + Duration::from_secs(5),
        );
        assert!(!sh.alive(t0, t0 + Duration::from_secs(5)));
    }

    #[test]
    fn acks_stopped_and_refusals_are_tracked() {
        let sh = Shared::default();
        let t = Instant::now();
        sh.handle_line(
            r#"{"type":"ack","cmd":"set_all_pwm","ok":true,"detail":"chassis_controls=2"}"#,
            t,
        );
        let a = sh
            .wait_ack("set_all_pwm", Duration::from_millis(10))
            .expect("ack");
        assert!(a.ok);
        assert_eq!(a.detail.as_deref(), Some("chassis_controls=2"));
        assert!(
            sh.wait_ack("set_all_pwm", Duration::from_millis(10))
                .is_none()
        );
        sh.handle_line(r#"{"type":"stopped","pwm_reset":true,"verified":true}"#, t);
        assert_eq!(
            sh.wait_stopped(Duration::from_millis(10)),
            Some((true, true))
        );
        sh.handle_line(r#"{"type":"refused","reason":"anti_cheat_active"}"#, t);
        assert_eq!(
            lock(&sh.inner).refused.as_deref(),
            Some("anti_cheat_active")
        );
        // garbage never panics and counts as liveness
        sh.handle_line("not json", t);
        sh.handle_line(r#"{"type":"brand_new_thing"}"#, t);
    }

    #[test]
    fn no_bridge_means_no_data_and_a_noop_stop() {
        let m = LhmBridgeManager::new(vec![]);
        assert!(m.latest().is_none());
        assert_eq!(m.check(Instant::now()), BridgeHealth::NotStarted);
        assert_eq!(m.stop(), StopOutcome::default());
        assert!(m.set_case_fans(Some(50)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_dead_bridge_clears_its_snapshot_and_is_reported() {
        // A stand-in "bridge": a batch file printing one snapshot, lingering, then exiting with code 4.
        let m = LhmBridgeManager::new(vec![]);
        let script =
            std::env::temp_dir().join(format!("ctrlmate-fake-bridge-{}.cmd", std::process::id()));
        std::fs::write(
            &script,
            format!(
                "@echo off
echo {SNAP}
ping -n 3 127.0.0.1 >nul
exit /b 4
"
            ),
        )
        .unwrap();
        let cmd = Command::new(&script);
        assert!(m.spawn(cmd, &script));
        let t0 = Instant::now();
        let mut got = false;
        while t0.elapsed() < Duration::from_secs(10) {
            if m.latest().is_some() {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(got, "snapshot never arrived");
        let mut health = BridgeHealth::Running;
        while t0.elapsed() < Duration::from_secs(15) {
            health = m.check(Instant::now());
            if health != BridgeHealth::Running {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(health, BridgeHealth::Exited(Some(4)));
        assert!(m.latest().is_none());
        assert!(!m.is_running());
        let _ = std::fs::remove_file(&script);
    }
}
