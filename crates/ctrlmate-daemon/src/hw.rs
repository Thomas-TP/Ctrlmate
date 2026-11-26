//! Hardware actor. The `HardwareCollector` (blocking NVML / HID / SMBus / WMI calls) lives on one
//! dedicated OS thread; async code talks to it through a channel and never holds a lock across
//! hardware work, so a slow poll can never stall the Tokio runtime, the IPC/HA servers or the lock
//! protecting daemon state.
//!
//! Lighting runs on its **own** thread over a [`LightingBackend`] handle (the HAL's `LightingHub`,
//! which shares no lock with sensor collection): a long `poll()` never delays a lighting frame. The
//! lighting thread only ticks while an animation is running (it sleeps otherwise). The cooling
//! watchdog runs on a third, HAL-owned thread with its own timer.

use ctrlmate_hal::{CollectorOptions, HardwareCollector, lighting::LightingHub};
use ctrlmate_protocol::{
    AllowlistedScript, CoolingPolicy, CoolingPreset, DetectedHidDevice, DeviceManifest,
    EntityState, LightState, OsUpdateState, RgbColor, RgbDeviceStatus, SecurityMode,
    SoftwareConflict, ThermalThresholds,
};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{error, info};

const FRAME: Duration = Duration::from_millis(16);
const IDLE_WAKE: Duration = Duration::from_secs(1);
const ANIMATION_TAIL: Duration = Duration::from_millis(400);

/// Slow-changing hardware facts, refreshed after every poll.
#[derive(Debug, Clone, Default)]
pub struct HwInfo {
    pub conflicts: Vec<SoftwareConflict>,
    pub hid_devices: Vec<DetectedHidDevice>,
    pub failsafe_active: bool,
    pub rgb_backends: Vec<String>,
    pub rgb_devices: Vec<RgbDeviceStatus>,
    pub deep_bridge_active: bool,
    /// Curves, thresholds, effective state and reason of the cooling watchdog.
    pub cooling: CoolingPolicy,
}

#[derive(Debug, Clone)]
pub struct PollOutput {
    pub states: Vec<EntityState>,
    /// The hardware the manifest depends on changed (new sensors, drives, zones): rebuild it.
    pub manifest_dirty: bool,
    pub info: HwInfo,
}

/// Lighting side of the hardware: independent of sensor collection.
pub trait LightingBackend: Send + Sync + 'static {
    fn apply(
        &self,
        entity_id: &str,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    ) -> Vec<(String, LightState)>;
    /// Advances transitions; `true` while frames were produced.
    fn tick(&self) -> bool;
}

impl LightingBackend for LightingHub {
    fn apply(
        &self,
        entity_id: &str,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    ) -> Vec<(String, LightState)> {
        self.apply_update(
            entity_id,
            is_on,
            brightness,
            rgb,
            color_temp_kelvin,
            transition_ms,
        )
    }

    fn tick(&self) -> bool {
        LightingHub::tick(self, Instant::now())
    }
}

/// Everything the daemon needs from the hardware layer. `RealHardware` wraps the HAL; tests use a
/// fake. All methods may block and are only ever called from the hardware thread.
pub trait HardwareBackend: Send + 'static {
    fn poll(&mut self) -> PollOutput;
    fn build_manifest(&mut self, scripts: &[AllowlistedScript]) -> DeviceManifest;
    /// Handle for the dedicated lighting thread.
    fn lighting(&self) -> Arc<dyn LightingBackend>;
    fn set_cooling_preset(&mut self, preset: CoolingPreset);
    fn set_thermal_thresholds(&mut self, thresholds: ThermalThresholds);
    fn set_security_mode(&mut self, mode: SecurityMode);
    fn rescan(&mut self);
    fn info(&self) -> HwInfo;
    fn os_updates(&self) -> OsUpdateState;
    fn set_os_updates(&mut self, state: OsUpdateState);
    fn set_os_updates_in_progress(&mut self, in_progress: bool);
}

/// Production backend over `ctrlmate_hal::HardwareCollector`.
pub struct RealHardware {
    col: HardwareCollector,
}

impl RealHardware {
    pub fn new(device_id: String, sw_version: String, opts: CollectorOptions) -> Self {
        Self {
            col: HardwareCollector::with_options(device_id, sw_version, opts),
        }
    }
}

impl HardwareBackend for RealHardware {
    fn poll(&mut self) -> PollOutput {
        let (states, manifest_dirty) = self.col.poll_states();
        PollOutput {
            states,
            manifest_dirty,
            info: self.info(),
        }
    }

    fn build_manifest(&mut self, scripts: &[AllowlistedScript]) -> DeviceManifest {
        self.col.build_manifest(scripts)
    }

    fn lighting(&self) -> Arc<dyn LightingBackend> {
        Arc::new(self.col.lighting())
    }

    fn set_cooling_preset(&mut self, preset: CoolingPreset) {
        self.col.set_cooling_preset(preset);
    }

    fn set_thermal_thresholds(&mut self, thresholds: ThermalThresholds) {
        self.col.update_thermal_thresholds(thresholds);
    }

    fn set_security_mode(&mut self, mode: SecurityMode) {
        self.col.set_security_mode(mode);
    }

    fn rescan(&mut self) {
        self.col.rescan_hardware();
    }

    fn info(&self) -> HwInfo {
        HwInfo {
            conflicts: self.col.detect_conflicts(),
            hid_devices: self.col.hid_devices().to_vec(),
            failsafe_active: self.col.is_thermal_failsafe_active(),
            rgb_backends: self.col.rgb_backend_names(),
            rgb_devices: self.col.rgb_device_statuses(),
            deep_bridge_active: self.col.deep_bridge_active(),
            cooling: self.col.cooling_policy(),
        }
    }

    fn os_updates(&self) -> OsUpdateState {
        self.col.os_updates().clone()
    }

    fn set_os_updates(&mut self, state: OsUpdateState) {
        self.col.set_os_updates(state);
    }

    fn set_os_updates_in_progress(&mut self, in_progress: bool) {
        self.col.set_os_updates_in_progress(in_progress);
    }
}

type Reply<T> = oneshot::Sender<T>;

enum Cmd {
    Poll(Reply<PollOutput>),
    Manifest(Vec<AllowlistedScript>, Reply<DeviceManifest>),
    Preset(CoolingPreset),
    Thermal(ThermalThresholds),
    SecurityMode(SecurityMode, Reply<()>),
    Rescan(Reply<()>),
    UpdatesSet(OsUpdateState),
    UpdatesProgress(bool, Reply<OsUpdateState>),
    /// Drops the backend (restoring firmware control) and ends the thread.
    Shutdown(Reply<()>),
}

enum LightCmd {
    Light {
        entity_id: String,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
        reply: Reply<Vec<(String, LightState)>>,
    },
    Shutdown(Reply<()>),
}

/// Handle to the hardware thread and the lighting thread.
pub struct HwHandle {
    tx: mpsc::Sender<Cmd>,
    light_tx: mpsc::Sender<LightCmd>,
    join: std::sync::Mutex<Vec<JoinHandle<()>>>,
}

impl HwHandle {
    /// Starts the actor threads. `factory` runs on the hardware thread (HID / NVML handles are
    /// created where they are used); the lighting thread starts right after it.
    pub fn spawn<B, F>(factory: F) -> std::io::Result<Self>
    where
        B: HardwareBackend,
        F: FnOnce() -> B + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let (light_tx, light_rx) = mpsc::channel();
        let hw_join = std::thread::Builder::new()
            .name("ctrlmate-hw".into())
            .spawn(move || {
                let backend = factory();
                let light = backend.lighting();
                let light_join = std::thread::Builder::new()
                    .name("ctrlmate-light".into())
                    .spawn(move || run_lighting(&*light, &light_rx));
                run(backend, &rx);
                if let Ok(j) = light_join {
                    let _ = j.join();
                }
            })?;
        Ok(Self {
            tx,
            light_tx,
            join: std::sync::Mutex::new(vec![hw_join]),
        })
    }

    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Option<T> {
        let (rtx, rrx) = oneshot::channel();
        self.tx.send(make(rtx)).ok()?;
        rrx.await.ok()
    }

    pub async fn poll(&self) -> Option<PollOutput> {
        self.call(Cmd::Poll).await
    }

    pub async fn build_manifest(&self, scripts: Vec<AllowlistedScript>) -> Option<DeviceManifest> {
        self.call(|r| Cmd::Manifest(scripts, r)).await
    }

    pub async fn apply_light(
        &self,
        entity_id: String,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    ) -> Option<Vec<(String, LightState)>> {
        let (reply, rrx) = oneshot::channel();
        self.light_tx
            .send(LightCmd::Light {
                entity_id,
                is_on,
                brightness,
                rgb,
                color_temp_kelvin,
                transition_ms,
                reply,
            })
            .ok()?;
        rrx.await.ok()
    }

    /// Fire and forget: preset changes are idempotent and re-sent on every state change.
    pub fn set_cooling_preset(&self, preset: CoolingPreset) {
        let _ = self.tx.send(Cmd::Preset(preset));
    }

    /// Runtime update of the watchdog thresholds (clamped by the cooling engine).
    pub fn set_thermal_thresholds(&self, t: ThermalThresholds) {
        let _ = self.tx.send(Cmd::Thermal(t));
    }

    pub async fn set_security_mode(&self, mode: SecurityMode) -> Option<()> {
        self.call(|r| Cmd::SecurityMode(mode, r)).await
    }

    pub async fn rescan(&self) -> Option<()> {
        self.call(Cmd::Rescan).await
    }

    pub fn set_os_updates(&self, state: OsUpdateState) {
        let _ = self.tx.send(Cmd::UpdatesSet(state));
    }

    pub async fn set_os_updates_in_progress(&self, in_progress: bool) -> Option<OsUpdateState> {
        self.call(|r| Cmd::UpdatesProgress(in_progress, r)).await
    }

    /// Stops lighting, restores hardware to firmware control (the backend's `Drop`) and joins the
    /// threads.
    pub async fn shutdown(&self) {
        let (r, rrx) = oneshot::channel();
        if self.light_tx.send(LightCmd::Shutdown(r)).is_ok() {
            let _ = rrx.await;
        }
        let _ = self.call(Cmd::Shutdown).await;
        let joins: Vec<_> = std::mem::take(
            &mut *self
                .join
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for j in joins {
            let _ = tokio::task::spawn_blocking(move || j.join()).await;
        }
    }
}

fn run_lighting(light: &dyn LightingBackend, rx: &mpsc::Receiver<LightCmd>) {
    let mut animating_until = Instant::now();
    let mut next_frame = Instant::now();
    loop {
        let now = Instant::now();
        let wait = if now < animating_until {
            next_frame.saturating_duration_since(now)
        } else {
            IDLE_WAKE
        };
        match rx.recv_timeout(wait) {
            Ok(LightCmd::Light {
                entity_id,
                is_on,
                brightness,
                rgb,
                color_temp_kelvin,
                transition_ms,
                reply,
            }) => {
                let out = light.apply(
                    &entity_id,
                    is_on,
                    brightness,
                    rgb,
                    color_temp_kelvin,
                    transition_ms,
                );
                let tail = Duration::from_millis(u64::from(transition_ms.unwrap_or(0)));
                animating_until = Instant::now() + tail + ANIMATION_TAIL;
                next_frame = Instant::now();
                let _ = reply.send(out);
            }
            Ok(LightCmd::Shutdown(r)) => {
                let _ = r.send(());
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        if now < animating_until && now >= next_frame {
            if light.tick() {
                // Frames are still flowing: keep ticking beyond the nominal window.
                animating_until = animating_until.max(now + ANIMATION_TAIL / 2);
            }
            next_frame = now + FRAME;
        }
    }
}

fn run<B: HardwareBackend>(mut backend: B, rx: &mpsc::Receiver<Cmd>) {
    loop {
        match rx.recv() {
            Ok(Cmd::Poll(r)) => {
                let _ = r.send(backend.poll());
            }
            Ok(Cmd::Manifest(scripts, r)) => {
                let _ = r.send(backend.build_manifest(&scripts));
            }
            Ok(Cmd::Preset(p)) => backend.set_cooling_preset(p),
            Ok(Cmd::Thermal(t)) => backend.set_thermal_thresholds(t),
            Ok(Cmd::SecurityMode(m, r)) => {
                backend.set_security_mode(m);
                let _ = r.send(());
            }
            Ok(Cmd::Rescan(r)) => {
                backend.rescan();
                let _ = r.send(());
            }
            Ok(Cmd::UpdatesSet(s)) => backend.set_os_updates(s),
            Ok(Cmd::UpdatesProgress(p, r)) => {
                backend.set_os_updates_in_progress(p);
                let _ = r.send(backend.os_updates());
            }
            Ok(Cmd::Shutdown(r)) => {
                info!("Hardware thread shutting down; restoring firmware control");
                drop(backend);
                let _ = r.send(());
                return;
            }
            Err(_) => {
                error!("Hardware command channel closed; stopping hardware thread");
                return;
            }
        }
    }
}

#[cfg(test)]
pub mod fake {
    //! Deterministic in-memory backend for daemon tests.

    use super::*;
    use chrono::Utc;
    use ctrlmate_protocol::{
        ChildDeviceInfo, DeviceCategory, EntityCategory, EntityDescriptor, EntityPlatform,
        EntityValue, HostDeviceInfo, LightCapabilities, PROTOCOL_VERSION,
    };
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct FakeLog {
        pub calls: Vec<String>,
        pub ticks: u32,
        pub poll_delay: Duration,
        pub extra_states: Vec<EntityState>,
        pub dropped: bool,
        /// Entity ids the fake poll leaves out (a sensor that disappeared).
        pub hide: Vec<String>,
        /// Thresholds last pushed by the daemon.
        pub thresholds: Option<ThermalThresholds>,
        /// The fake poll reports `manifest_dirty` once.
        pub dirty_once: bool,
    }

    /// Lighting half of the fake: shares the log, no lock with the poll except the log itself.
    pub struct FakeLighting {
        log: Arc<Mutex<FakeLog>>,
        light: Mutex<LightState>,
    }

    impl LightingBackend for FakeLighting {
        fn apply(
            &self,
            entity_id: &str,
            is_on: Option<bool>,
            brightness: Option<u8>,
            _rgb: Option<RgbColor>,
            _kelvin: Option<u16>,
            _transition_ms: Option<u32>,
        ) -> Vec<(String, LightState)> {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .calls
                .push(format!("light:{entity_id}"));
            if entity_id != "light_global" {
                return vec![];
            }
            let mut l = self
                .light
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(on) = is_on {
                l.is_on = on;
            }
            if let Some(b) = brightness {
                l.brightness = b;
            }
            vec![("light_global".into(), l.clone())]
        }

        fn tick(&self) -> bool {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .ticks += 1;
            false
        }
    }

    pub struct FakeHardware {
        pub log: Arc<Mutex<FakeLog>>,
        light: Arc<FakeLighting>,
        updates: OsUpdateState,
    }

    impl FakeHardware {
        pub fn new(log: Arc<Mutex<FakeLog>>) -> Self {
            Self {
                light: Arc::new(FakeLighting {
                    log: Arc::clone(&log),
                    light: Mutex::new(LightState::default()),
                }),
                log,
                updates: OsUpdateState::default(),
            }
        }

        fn record(&self, s: impl Into<String>) {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .calls
                .push(s.into());
        }
    }

    impl Drop for FakeHardware {
        fn drop(&mut self) {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .dropped = true;
        }
    }

    pub fn state(id: &str, value: EntityValue) -> EntityState {
        EntityState {
            id: id.into(),
            value,
            attributes: BTreeMap::new(),
            updated_at: Utc::now(),
        }
    }

    impl HardwareBackend for FakeHardware {
        fn poll(&mut self) -> PollOutput {
            let (delay, extra) = {
                let l = self
                    .log
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (l.poll_delay, l.extra_states.clone())
            };
            std::thread::sleep(delay);
            self.record("poll");
            let mut states = vec![
                state("cpu_0_load", EntityValue::Float(12.0)),
                state("gpu_0_temperature", EntityValue::Float(51.0)),
                state(
                    "light_global",
                    EntityValue::Light(
                        self.light
                            .light
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone(),
                    ),
                ),
                // Session-bound entities the real HAL fabricates in session 0.
                state("master_volume", EntityValue::Float(50.0)),
                state("audio_mute", EntityValue::Bool(false)),
                state("audio_output", EntityValue::Text("Default Output".into())),
                state("gaming_mode", EntityValue::Bool(false)),
                state("active_application", EntityValue::Text(String::new())),
                state("microphone_in_use", EntityValue::Bool(false)),
                state("webcam_in_use", EntityValue::Bool(false)),
            ];
            states.extend(extra);
            let hide = self
                .log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .hide
                .clone();
            states.retain(|s| !hide.contains(&s.id));
            let dirty = std::mem::take(
                &mut self
                    .log
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .dirty_once,
            );
            PollOutput {
                states,
                manifest_dirty: dirty,
                info: self.info(),
            }
        }

        fn build_manifest(&mut self, scripts: &[AllowlistedScript]) -> DeviceManifest {
            self.record(format!("manifest:{}", scripts.len()));
            let mut entities = vec![EntityDescriptor {
                id: "light_global".into(),
                name: "Light".into(),
                platform: EntityPlatform::Light,
                child_device_id: None,
                device_class: None,
                state_class: None,
                unit_of_measurement: None,
                icon: None,
                entity_category: None::<EntityCategory>,
                suggested_display_precision: None,
                options: vec![],
                number_range: None,
                light_caps: Some(LightCapabilities::default()),
                exposed: true,
            }];
            for s in scripts {
                entities.push(EntityDescriptor {
                    id: format!("script_{}", s.id),
                    name: s.name.clone(),
                    platform: EntityPlatform::Button,
                    child_device_id: None,
                    device_class: None,
                    state_class: None,
                    unit_of_measurement: None,
                    icon: None,
                    entity_category: None,
                    suggested_display_precision: None,
                    options: vec![],
                    number_range: None,
                    light_caps: None,
                    exposed: true,
                });
            }
            DeviceManifest {
                protocol_version: PROTOCOL_VERSION,
                host: HostDeviceInfo {
                    device_id: "fake-device".into(),
                    name: "FAKE-PC".into(),
                    manufacturer: "Acme".into(),
                    model: "Rig".into(),
                    os_name: "Windows".into(),
                    os_version: "11".into(),
                    sw_version: "0.0.0-test".into(),
                    mac_address: Some("AA:BB:CC:DD:EE:FF".into()),
                    security_mode: SecurityMode::ZeroKernelSafe,
                },
                child_devices: vec![ChildDeviceInfo {
                    id: "cpu_0".into(),
                    name: "CPU".into(),
                    category: DeviceCategory::Cpu,
                    manufacturer: None,
                    model: None,
                    hw_version: None,
                }],
                entities,
            }
        }

        fn lighting(&self) -> Arc<dyn LightingBackend> {
            Arc::clone(&self.light) as Arc<dyn LightingBackend>
        }

        fn set_thermal_thresholds(&mut self, t: ThermalThresholds) {
            self.record(format!("thermal:{}", t.cpu_critical_c));
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .thresholds = Some(t);
        }

        fn set_cooling_preset(&mut self, preset: CoolingPreset) {
            self.record(format!("preset:{}", preset.as_ha_option()));
        }

        fn set_security_mode(&mut self, mode: SecurityMode) {
            self.record(format!("security:{mode:?}"));
        }

        fn rescan(&mut self) {
            self.record("rescan");
        }

        fn info(&self) -> HwInfo {
            let mut info = HwInfo::default();
            if let Some(t) = self
                .log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .thresholds
            {
                info.cooling.thresholds = ctrlmate_hal::sanitize_thresholds(t);
            }
            info
        }

        fn os_updates(&self) -> OsUpdateState {
            self.updates.clone()
        }

        fn set_os_updates(&mut self, state: OsUpdateState) {
            self.updates = state;
        }

        fn set_os_updates_in_progress(&mut self, in_progress: bool) {
            self.updates.in_progress = in_progress;
        }
    }

    pub fn spawn_fake() -> (HwHandle, Arc<Mutex<FakeLog>>) {
        let log = Arc::new(Mutex::new(FakeLog::default()));
        let l2 = Arc::clone(&log);
        let h = HwHandle::spawn(move || FakeHardware::new(l2)).expect("spawn fake hardware");
        (h, log)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[tokio::test]
    async fn commands_roundtrip_and_shutdown_drops_backend() {
        let (hw, log) = spawn_fake();
        let out = hw.poll().await.expect("poll");
        assert!(out.states.iter().any(|s| s.id == "cpu_0_load"));
        let m = hw.build_manifest(vec![]).await.expect("manifest");
        assert_eq!(m.host.name, "FAKE-PC");
        let r = hw
            .apply_light(
                "light_global".into(),
                Some(false),
                Some(10),
                None,
                None,
                None,
            )
            .await
            .expect("light");
        assert_eq!(r.len(), 1);
        assert!(!r[0].1.is_on);
        hw.set_cooling_preset(CoolingPreset::Gaming);
        hw.shutdown().await;
        {
            let l = log.lock().unwrap();
            assert!(
                l.dropped,
                "backend must be dropped (HAL restores firmware control)"
            );
            assert!(l.calls.contains(&"preset:gaming".to_string()));
        }
        // Commands after shutdown fail cleanly instead of hanging.
        assert!(hw.poll().await.is_none());
    }

    #[tokio::test]
    async fn blocking_poll_does_not_block_the_runtime() {
        let (hw, log) = spawn_fake();
        log.lock().unwrap().poll_delay = Duration::from_millis(400);
        let hw = std::sync::Arc::new(hw);
        let h2 = std::sync::Arc::clone(&hw);
        let poll = tokio::spawn(async move { h2.poll().await });
        // The single-threaded test runtime stays responsive while the hardware thread sleeps.
        let started = Instant::now();
        let mut ticks = 0;
        while started.elapsed() < Duration::from_millis(250) {
            tokio::time::sleep(Duration::from_millis(10)).await;
            ticks += 1;
        }
        assert!(ticks >= 10, "runtime was blocked ({ticks} ticks)");
        assert!(poll.await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_long_poll_does_not_delay_lighting() {
        let (hw, log) = spawn_fake();
        log.lock().unwrap().poll_delay = Duration::from_millis(900);
        let hw = std::sync::Arc::new(hw);
        let h2 = std::sync::Arc::clone(&hw);
        let poll = tokio::spawn(async move { h2.poll().await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        // The sensor thread is stuck in poll(); a light command must still answer promptly and
        // frames must keep ticking.
        let t = Instant::now();
        hw.apply_light(
            "light_global".into(),
            Some(true),
            None,
            None,
            None,
            Some(400),
        )
        .await
        .expect("light");
        assert!(
            t.elapsed() < Duration::from_millis(300),
            "lighting waited for the poll: {:?}",
            t.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            log.lock().unwrap().ticks >= 5,
            "frames stalled during the poll"
        );
        assert!(poll.await.unwrap().is_some());
    }

    #[tokio::test]
    async fn lighting_ticks_only_while_animating() {
        let (hw, log) = spawn_fake();
        // Idle: no ticks at all.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(log.lock().unwrap().ticks, 0);
        hw.apply_light(
            "light_global".into(),
            Some(true),
            None,
            None,
            None,
            Some(100),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let during = log.lock().unwrap().ticks;
        assert!(during >= 5, "expected ~60 FPS ticks, got {during}");
        tokio::time::sleep(Duration::from_millis(900)).await;
        let after = log.lock().unwrap().ticks;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            log.lock().unwrap().ticks,
            after,
            "ticks must stop when idle"
        );
    }
}
