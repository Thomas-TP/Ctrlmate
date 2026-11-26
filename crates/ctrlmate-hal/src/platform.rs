//! The supervisor: anti-cheat gating, bridge lifecycle and RGB (re)discovery.
//!
//! One thread, one scan every [`SCAN_EVERY`] with a *fresh* process list. Discovery and bridge
//! start / stop happen only on state transitions (anti-cheat appeared / is gone for 30 s, security
//! mode changed, explicit rescan, bridge died), never on a timer.

use crate::anticheat::{self, AntiCheatGate, AntiCheatScanner, GateEvent};
use crate::conflicts;
use crate::lhm_bridge::{BridgeHealth, LhmBridgeManager};
use crate::lighting::LightingHub;
use crate::thermal::CoolingRuntime;
use ctrlmate_protocol::{SecurityMode, SoftwareConflict};
use ctrlmate_rgb::backends::{DiscoverOptions, RgbHardwareDispatcher};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

pub const SCAN_EVERY: Duration = Duration::from_secs(2);
/// Minimum time between two automatic bridge restarts.
pub const RESTART_BACKOFF: Duration = Duration::from_secs(15);
/// Automatic restarts before giving up until the next mode / anti-cheat transition.
pub const MAX_AUTO_RESTARTS: u32 = 5;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the supervisor needs to decide and act. Everything behind it is interior-mutable.
pub struct Platform {
    pub bridge: Arc<LhmBridgeManager>,
    pub lighting: LightingHub,
    pub cooling: Arc<CoolingRuntime>,
    mode: Mutex<SecurityMode>,
    gate: Mutex<AntiCheatGate>,
    scanner: Mutex<AntiCheatScanner>,
    conflicts: Mutex<Vec<SoftwareConflict>>,
    discover_base: DiscoverOptions,
    /// Serialises discovery / start / stop between the supervisor and API calls.
    reconcile: Mutex<()>,
    restarts: Mutex<(u32, Option<Instant>)>,
}

impl Platform {
    pub fn new(
        bridge: Arc<LhmBridgeManager>,
        lighting: LightingHub,
        cooling: Arc<CoolingRuntime>,
        mode: SecurityMode,
        discover_base: DiscoverOptions,
    ) -> Self {
        Self {
            bridge,
            lighting,
            cooling,
            mode: Mutex::new(mode),
            gate: Mutex::new(AntiCheatGate::new()),
            scanner: Mutex::new(AntiCheatScanner::new()),
            conflicts: Mutex::new(Vec::new()),
            discover_base,
            reconcile: Mutex::new(()),
            restarts: Mutex::new((0, None)),
        }
    }

    pub fn mode(&self) -> SecurityMode {
        *lock(&self.mode)
    }

    pub fn anti_cheat_paused(&self) -> bool {
        lock(&self.gate).is_paused()
    }

    pub fn conflicts(&self) -> Vec<SoftwareConflict> {
        lock(&self.conflicts).clone()
    }

    fn deep_wanted(&self) -> bool {
        self.mode() == SecurityMode::DeepHardwarePawnIo && !self.anti_cheat_paused()
    }

    /// Discovery options for the current state (kernel SMBus / OpenRGB off while paused).
    pub fn discover_options(&self) -> DiscoverOptions {
        anticheat::discover_options(
            self.mode() == SecurityMode::DeepHardwarePawnIo,
            self.anti_cheat_paused(),
            &self.discover_base,
        )
    }

    /// Runs RGB discovery with the current options and installs the result (blocking, slow).
    pub fn rediscover_rgb(&self) {
        let opts = self.discover_options();
        let d = RgbHardwareDispatcher::discover_with(&opts);
        self.lighting.install_hardware(d);
    }

    /// Scans processes + services now and updates conflicts and the gate.
    pub fn scan(&self, now: Instant) -> GateEvent {
        let (acs, names) = lock(&self.scanner).scan_with_names();
        *lock(&self.conflicts) = conflicts::conflicts_from(&names, &acs);
        lock(&self.gate).update(now, !acs.is_empty())
    }

    /// Brings bridge and RGB in line with `mode` / the gate. `rediscover` forces an RGB rediscovery.
    pub fn reconcile(&self, rediscover: bool) {
        let _g = lock(&self.reconcile);
        let wanted = self.deep_wanted();
        self.cooling.set_bridge_wanted(wanted);
        if wanted {
            if !self.bridge.is_running() && self.bridge.start() {
                self.cooling.bridge_started();
            }
        } else if self.bridge.is_running() {
            let out = self.bridge.stop();
            info!("Deep hardware bridge stopped ({out:?})");
        }
        if rediscover {
            self.rediscover_rgb();
        }
    }

    pub fn set_mode(&self, mode: SecurityMode) {
        *lock(&self.mode) = mode;
        *lock(&self.restarts) = (0, None);
        self.reconcile(true);
    }

    /// One supervisor iteration.
    pub fn tick(&self, now: Instant) {
        match self.scan(now) {
            GateEvent::Paused => {
                warn!(
                    "Anti-cheat detected: stopping the deep-hardware bridge and every SMBus / PawnIO path"
                );
                self.reconcile(true);
            }
            GateEvent::Resumed => {
                info!("Anti-cheat gone for 30 s: deep hardware may resume");
                *lock(&self.restarts) = (0, None);
                self.reconcile(true);
            }
            GateEvent::None => self.watch_bridge(now),
        }
    }

    /// Liveness of the bridge process: a dead or hung bridge is stopped (its snapshot is cleared by the
    /// manager, so the cooling engine sees the sensors go stale) and restarted with a backoff.
    fn watch_bridge(&self, now: Instant) {
        if !self.deep_wanted() {
            return;
        }
        let health = self.bridge.check(now);
        match health {
            BridgeHealth::Running => return,
            BridgeHealth::NotStarted => {}
            BridgeHealth::Exited(code) => {
                // 3 = refused (anti-cheat / no driver), 4 = anti-cheat appeared: do not hammer.
                if matches!(code, Some(3 | 4)) {
                    warn!(
                        "Bridge ended with code {code:?}; not restarting until the next scan transition"
                    );
                    return;
                }
            }
            BridgeHealth::Unresponsive => {
                error!("Bridge unresponsive: stopping it");
                let _ = self.bridge.stop();
            }
        }
        let mut r = lock(&self.restarts);
        if r.0 >= MAX_AUTO_RESTARTS {
            return;
        }
        if r.1
            .is_some_and(|t| now.saturating_duration_since(t) < RESTART_BACKOFF)
        {
            return;
        }
        r.0 += 1;
        r.1 = Some(now);
        drop(r);
        let _g = lock(&self.reconcile);
        if self.deep_wanted() && !self.bridge.is_running() && self.bridge.start() {
            self.cooling.bridge_started();
        }
    }
}

/// The supervisor thread.
pub struct Supervisor {
    stop: Arc<(Mutex<bool>, Condvar)>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl Supervisor {
    pub fn spawn(platform: Arc<Platform>) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let s2 = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("ctrlmate-supervisor".into())
            .spawn(move || {
                loop {
                    let (m, cv) = &*s2;
                    let g = lock(m);
                    let (g, _) = cv
                        .wait_timeout(g, SCAN_EVERY)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if *g {
                        break;
                    }
                    drop(g);
                    platform.tick(Instant::now());
                }
            })
            .ok();
        Self {
            stop,
            join: Mutex::new(join),
        }
    }

    pub fn stop(&self) {
        *lock(&self.stop.0) = true;
        self.stop.1.notify_all();
        if let Some(j) = lock(&self.join).take() {
            let _ = j.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thermal::mock::{MockIo, Script};

    fn platform(mode: SecurityMode) -> Platform {
        let io = MockIo(Arc::new(Mutex::new(Script::default())));
        let cooling = Arc::new(CoolingRuntime::spawn(
            Box::new(io),
            Default::default(),
            Duration::from_millis(50),
        ));
        Platform::new(
            Arc::new(LhmBridgeManager::new(vec![])),
            LightingHub::new(),
            cooling,
            mode,
            DiscoverOptions {
                enable_openrgb: false,
                ..DiscoverOptions::default()
            },
        )
    }

    #[test]
    fn options_follow_mode_and_gate() {
        let p = platform(SecurityMode::ZeroKernelSafe);
        assert!(!p.discover_options().allow_kernel_smbus);
        let p = platform(SecurityMode::DeepHardwarePawnIo);
        assert!(p.discover_options().allow_kernel_smbus);
        // An anti-cheat appears.
        let t0 = Instant::now();
        assert_eq!(lock(&p.gate).update(t0, true), GateEvent::Paused);
        let o = p.discover_options();
        assert!(!o.allow_kernel_smbus && !o.enable_openrgb);
        assert!(p.anti_cheat_paused());
        assert!(!p.deep_wanted());
    }

    #[test]
    fn zero_kernel_never_wants_the_bridge() {
        let p = platform(SecurityMode::ZeroKernelSafe);
        assert!(!p.deep_wanted());
        p.reconcile(false);
        assert!(!p.bridge.is_running());
    }

    #[test]
    fn scanning_updates_conflicts_without_panicking() {
        let p = platform(SecurityMode::ZeroKernelSafe);
        let _ = p.scan(Instant::now());
        let _ = p.conflicts();
    }
}
