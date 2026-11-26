//! Lighting, decoupled from sensor collection.
//!
//! [`LightingHub`] is a cheap, cloneable handle around the logical colour engine and the RGB
//! hardware dispatcher. It has its own locks, so a slow sensor poll never delays a lighting frame
//! and the daemon's lighting thread never touches the lock the sensor collector uses.
//! Frames go out through the dispatcher's non-blocking `submit_frame`.

use ctrlmate_protocol::{LightState, RgbColor, RgbDeviceStatus};
use ctrlmate_rgb::backends::{BackendStats, RgbHardwareDispatcher};
use ctrlmate_rgb::{LightingEngine, ZoneInfo};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};
use std::time::Instant;

struct Shared {
    engine: Mutex<LightingEngine>,
    rgb: RwLock<RgbHardwareDispatcher>,
}

/// Cloneable lighting handle.
#[derive(Clone)]
pub struct LightingHub {
    shared: Arc<Shared>,
}

impl Default for LightingHub {
    fn default() -> Self {
        Self::new()
    }
}

impl LightingHub {
    /// A hub without hardware (only the virtual master zone).
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                engine: Mutex::new(LightingEngine::new()),
                rgb: RwLock::new(RgbHardwareDispatcher::empty()),
            }),
        }
    }

    fn engine(&self) -> MutexGuard<'_, LightingEngine> {
        self.shared
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn rgb(&self) -> RwLockReadGuard<'_, RgbHardwareDispatcher> {
        self.shared
            .rgb
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Installs freshly discovered hardware: the engine's zones are rebuilt from the *controllable*
    /// zones only (no guessed zones), the state of zones that still exist is kept, and the current
    /// colours are pushed to the new backends. The old dispatcher is dropped outside any lock.
    pub fn install_hardware(&self, new: RgbHardwareDispatcher) {
        let zones = new.controllable_zones();
        {
            let mut e = self.engine();
            let old_states = e.all_zones().clone();
            e.clear_zones();
            for z in &zones {
                let initial = old_states.get(&z.id).cloned().unwrap_or_default();
                e.register_zone_with_caps(z.id.clone(), initial, z.capabilities);
            }
            e.invalidate_output();
        }
        let old = {
            let mut w = self
                .shared
                .rgb
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *w, new)
        };
        drop(old);
        // Bring the new backends to the colours the engine currently shows.
        let frames = self.engine().current_frames(Instant::now());
        let rgb = self.rgb();
        for (zone, color) in frames {
            rgb.submit_frame(&zone, color);
        }
    }

    /// Applies a Home Assistant / UI light update (see `LightingEngine::apply_update`).
    pub fn apply_update(
        &self,
        entity_id: &str,
        is_on: Option<bool>,
        brightness: Option<u8>,
        rgb: Option<RgbColor>,
        color_temp_kelvin: Option<u16>,
        transition_ms: Option<u32>,
    ) -> Vec<(String, LightState)> {
        self.engine().apply_update(
            entity_id,
            is_on,
            brightness,
            rgb,
            color_temp_kelvin,
            transition_ms,
        )
    }

    /// Advances transitions and submits the frames; `true` while frames were produced.
    pub fn tick(&self, now: Instant) -> bool {
        let frames = self.engine().tick_transitions(now);
        if frames.is_empty() {
            return false;
        }
        let rgb = self.rgb();
        for (zone, color) in frames {
            rgb.submit_frame(&zone, color);
        }
        true
    }

    pub fn is_animating(&self) -> bool {
        self.engine().is_animating()
    }

    /// Master plus every registered zone.
    pub fn states(&self) -> Vec<(String, LightState)> {
        self.engine()
            .all_zones()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn has_zone(&self, id: &str) -> bool {
        self.engine().zone_state(id).is_some()
    }

    /// Real zones including informational ones.
    pub fn zones(&self) -> Vec<ZoneInfo> {
        self.rgb().zones().to_vec()
    }

    pub fn device_statuses(&self) -> Vec<RgbDeviceStatus> {
        self.rgb().device_statuses().to_vec()
    }

    pub fn backend_names(&self) -> Vec<String> {
        self.rgb().active_backend_names()
    }

    pub fn backend_stats(&self) -> Vec<BackendStats> {
        self.rgb().backend_stats()
    }

    pub fn has_ram_rgb(&self) -> bool {
        self.rgb().has_ram_rgb()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use ctrlmate_rgb::backends::LedSink;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Counter(Arc<AtomicUsize>, Arc<Mutex<Option<RgbColor>>>);
    impl LedSink for Counter {
        fn name(&self) -> String {
            "counter".into()
        }
        fn write(&mut self, c: RgbColor) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            *self.1.lock().unwrap() = Some(c);
            Ok(())
        }
        fn min_interval(&self) -> Duration {
            Duration::from_millis(1)
        }
    }

    fn dispatcher() -> (
        RgbHardwareDispatcher,
        Arc<AtomicUsize>,
        Arc<Mutex<Option<RgbColor>>>,
    ) {
        let n = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(Mutex::new(None));
        let mut d = RgbHardwareDispatcher::empty();
        d.add_sink("light_fans", Box::new(Counter(n.clone(), last.clone())));
        d.add_zone(ZoneInfo::controllable(
            "light_fans",
            "fans",
            "test",
            "usb",
            8,
        ));
        d.add_zone(ZoneInfo::uncontrollable(
            "light_storage",
            "ssd",
            "samsung",
            "nvme",
            "no_public_protocol",
        ));
        (d, n, last)
    }

    #[test]
    fn only_real_controllable_zones_exist_and_no_phantoms() {
        let hub = LightingHub::new();
        assert!(hub.has_zone("light_global"));
        for phantom in [
            "light_gpu",
            "light_fans",
            "light_cooler",
            "light_ram",
            "light_motherboard",
        ] {
            assert!(!hub.has_zone(phantom), "{phantom}");
        }
        let (d, _, _) = dispatcher();
        hub.install_hardware(d);
        assert!(hub.has_zone("light_fans"));
        assert!(
            !hub.has_zone("light_storage"),
            "informational zones never enter the engine"
        );
        assert_eq!(hub.zones().len(), 2);
    }

    #[test]
    fn frames_reach_the_hardware_and_state_survives_a_rescan() {
        let hub = LightingHub::new();
        let (d, n, last) = dispatcher();
        hub.install_hardware(d);
        hub.apply_update(
            "light_fans",
            Some(true),
            Some(255),
            Some(RgbColor { r: 255, g: 0, b: 0 }),
            None,
            Some(10),
        );
        let t0 = Instant::now();
        let mut produced = false;
        while t0.elapsed() < Duration::from_secs(2) && hub.is_animating() {
            produced |= hub.tick(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(produced);
        std::thread::sleep(Duration::from_millis(100));
        assert!(n.load(Ordering::SeqCst) > 0);
        let c = last.lock().unwrap().unwrap();
        assert!(c.r > 0 && c.g == 0 && c.b == 0, "{c:?}");

        // Rescan: same zone, new dispatcher: the user's colour is kept and re-sent.
        let (d2, n2, last2) = dispatcher();
        hub.install_hardware(d2);
        let st = hub
            .states()
            .into_iter()
            .find(|(id, _)| id == "light_fans")
            .unwrap()
            .1;
        assert_eq!(st.rgb, RgbColor { r: 255, g: 0, b: 0 });
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            n2.load(Ordering::SeqCst) > 0,
            "new backend must receive the current colour"
        );
        assert!(last2.lock().unwrap().is_some());
    }

    #[test]
    fn ticking_never_needs_the_sensor_side() {
        // The hub is independent of any collector: two clones tick concurrently with an update.
        let hub = LightingHub::new();
        let h2 = hub.clone();
        let t = std::thread::spawn(move || {
            for _ in 0..50 {
                h2.tick(Instant::now());
            }
        });
        for _ in 0..50 {
            hub.apply_update("light_global", Some(true), Some(100), None, None, Some(1));
        }
        t.join().unwrap();
    }
}
