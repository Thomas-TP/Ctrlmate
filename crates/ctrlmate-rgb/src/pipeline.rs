//! Per-backend frame pipeline.
//!
//! The lighting engine produces frames at 60 FPS, but the hardware cannot follow: an ENE DIMM write
//! over SMBus takes tens of milliseconds, a Gigabyte HID report a few. Every backend therefore runs
//! on its **own worker thread** fed through a *latest-frame-wins* slot:
//!
//! * [`BackendWorker::submit`] never blocks on hardware (it only takes a short mutex);
//! * frames submitted while the worker is busy replace the pending one (counted as *coalesced*);
//! * the worker never writes faster than the sink's [`LedSink::min_interval`];
//! * write failures are counted, retried with exponential backoff and logged at a limited rate.

use anyhow::Result;
use ctrlmate_protocol::RgbColor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Locks a mutex, recovering from poisoning (the protected data is always plain state).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A device (or group of devices) that displays one uniform colour.
pub trait LedSink: Send {
    fn name(&self) -> String;
    /// Writes one frame. Implementations may skip identical frames internally, but must report real errors.
    fn write(&mut self, color: RgbColor) -> Result<()>;
    /// Minimum delay between two writes (per-backend maximum rate).
    fn min_interval(&self) -> Duration;
    /// Called once when the worker stops.
    fn shutdown(&mut self) {}
}

#[derive(Default)]
struct SlotState {
    pending: Option<RgbColor>,
    stop: bool,
}

/// Latest-frame-wins slot shared between the producer (engine tick) and one worker.
#[derive(Default)]
pub struct FrameSlot {
    state: Mutex<SlotState>,
    cv: Condvar,
    submitted: AtomicU64,
    coalesced: AtomicU64,
}

impl FrameSlot {
    /// Replaces the pending frame. Never blocks on hardware.
    pub fn submit(&self, color: RgbColor) {
        self.submitted.fetch_add(1, Ordering::Relaxed);
        let mut st = lock(&self.state);
        if st.pending.replace(color).is_some() {
            self.coalesced.fetch_add(1, Ordering::Relaxed);
        }
        drop(st);
        self.cv.notify_one();
    }

    fn take(&self) -> Option<RgbColor> {
        lock(&self.state).pending.take()
    }

    /// Waits for a frame. `Err(())` means stop was requested.
    fn wait_frame(&self, timeout: Duration) -> Result<Option<RgbColor>, ()> {
        let mut st = lock(&self.state);
        if st.stop {
            return Err(());
        }
        if let Some(c) = st.pending.take() {
            return Ok(Some(c));
        }
        let (mut st, _) = self
            .cv
            .wait_timeout(st, timeout)
            .unwrap_or_else(|e| e.into_inner());
        if st.stop {
            return Err(());
        }
        Ok(st.pending.take())
    }

    /// Interruptible sleep; returns `false` when stop was requested.
    fn sleep(&self, dur: Duration) -> bool {
        let deadline = Instant::now() + dur;
        let mut st = lock(&self.state);
        loop {
            if st.stop {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            let (g, _) = self
                .cv
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            st = g;
        }
    }

    fn request_stop(&self) {
        lock(&self.state).stop = true;
        self.cv.notify_all();
    }
}

/// Counters exposed for diagnostics.
#[derive(Debug, Default)]
pub struct BackendCounters {
    pub written: AtomicU64,
    pub failures: AtomicU64,
    pub last_error: Mutex<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendStats {
    pub name: String,
    pub frames_submitted: u64,
    pub frames_written: u64,
    pub frames_coalesced: u64,
    pub failures: u64,
    pub last_error: Option<String>,
}

const IDLE_WAKEUP: Duration = Duration::from_millis(500);
const BACKOFF_START: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
const LOG_INTERVAL: Duration = Duration::from_secs(30);

/// One worker thread driving one [`LedSink`].
pub struct BackendWorker {
    name: String,
    slot: Arc<FrameSlot>,
    counters: Arc<BackendCounters>,
    handle: Option<JoinHandle<()>>,
}

impl BackendWorker {
    pub fn spawn(mut sink: Box<dyn LedSink>) -> Self {
        let name = sink.name();
        let slot = Arc::new(FrameSlot::default());
        let counters = Arc::new(BackendCounters::default());
        let (w_slot, w_counters, w_name) = (slot.clone(), counters.clone(), name.clone());
        let handle = std::thread::Builder::new()
            .name(format!("rgb-{}", name.chars().take(24).collect::<String>()))
            .spawn(move || worker_loop(sink.as_mut(), &w_slot, &w_counters, &w_name))
            .ok();
        Self {
            name,
            slot,
            counters,
            handle,
        }
    }

    /// Non-blocking, latest-wins.
    pub fn submit(&self, color: RgbColor) {
        self.slot.submit(color);
    }

    pub fn stats(&self) -> BackendStats {
        BackendStats {
            name: self.name.clone(),
            frames_submitted: self.slot.submitted.load(Ordering::Relaxed),
            frames_written: self.counters.written.load(Ordering::Relaxed),
            frames_coalesced: self.slot.coalesced.load(Ordering::Relaxed),
            failures: self.counters.failures.load(Ordering::Relaxed),
            last_error: lock(&self.counters.last_error).clone(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for BackendWorker {
    fn drop(&mut self) {
        self.slot.request_stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn worker_loop(sink: &mut dyn LedSink, slot: &FrameSlot, counters: &BackendCounters, name: &str) {
    let mut last_write: Option<Instant> = None;
    let mut backoff = BACKOFF_START;
    let mut last_log: Option<Instant> = None;
    let mut suppressed: u64 = 0;

    loop {
        let first = match slot.wait_frame(IDLE_WAKEUP) {
            Err(()) => break,
            Ok(None) => continue,
            Ok(Some(c)) => c,
        };

        // Per-backend maximum rate: sleep, letting newer frames overwrite the pending one.
        if let Some(t) = last_write {
            let min = sink.min_interval();
            let elapsed = t.elapsed();
            if elapsed < min && !slot.sleep(min - elapsed) {
                break;
            }
        }
        let color = slot.take().unwrap_or(first);

        let result = sink.write(color);
        last_write = Some(Instant::now());
        match result {
            Ok(()) => {
                counters.written.fetch_add(1, Ordering::Relaxed);
                if backoff != BACKOFF_START {
                    debug!("RGB backend '{name}' recovered");
                }
                backoff = BACKOFF_START;
            }
            Err(err) => {
                counters.failures.fetch_add(1, Ordering::Relaxed);
                *lock(&counters.last_error) = Some(format!("{err:#}"));
                let due = last_log.is_none_or(|t| t.elapsed() >= LOG_INTERVAL);
                if due {
                    warn!(
                        "RGB backend '{name}' write failed: {err:#} ({suppressed} similar error(s) suppressed)"
                    );
                    last_log = Some(Instant::now());
                    suppressed = 0;
                } else {
                    suppressed += 1;
                }
                // Keep the failed colour pending (unless a newer frame arrived) so it is retried.
                {
                    let mut st = lock(&slot.state);
                    if st.pending.is_none() {
                        st.pending = Some(color);
                    }
                }
                if !slot.sleep(backoff) {
                    break;
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
    sink.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::sync::atomic::AtomicBool;

    struct SlowSink {
        delay: Duration,
        log: Arc<Mutex<Vec<RgbColor>>>,
        fail_first: Arc<AtomicBool>,
    }

    impl LedSink for SlowSink {
        fn name(&self) -> String {
            "slow".into()
        }
        fn write(&mut self, color: RgbColor) -> Result<()> {
            std::thread::sleep(self.delay);
            if self.fail_first.swap(false, Ordering::SeqCst) {
                return Err(anyhow!("boom"));
            }
            lock(&self.log).push(color);
            Ok(())
        }
        fn min_interval(&self) -> Duration {
            Duration::from_millis(10)
        }
    }

    fn c(v: u8) -> RgbColor {
        RgbColor { r: v, g: v, b: v }
    }

    #[test]
    fn submit_is_non_blocking_and_latest_frame_wins() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let worker = BackendWorker::spawn(Box::new(SlowSink {
            delay: Duration::from_millis(40),
            log: log.clone(),
            fail_first: Arc::new(AtomicBool::new(false)),
        }));
        let start = Instant::now();
        for i in 0..=200u8 {
            worker.submit(c(i));
            std::thread::sleep(Duration::from_micros(200));
        }
        assert!(start.elapsed() < Duration::from_millis(500));
        std::thread::sleep(Duration::from_millis(250));
        let stats = worker.stats();
        assert_eq!(stats.frames_submitted, 201);
        assert!(
            stats.frames_written < 60,
            "written {}",
            stats.frames_written
        );
        assert!(stats.frames_coalesced > 100);
        assert_eq!(lock(&log).last().copied(), Some(c(200)));
    }

    #[test]
    fn failures_are_counted_and_retried() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let worker = BackendWorker::spawn(Box::new(SlowSink {
            delay: Duration::from_millis(1),
            log: log.clone(),
            fail_first: Arc::new(AtomicBool::new(true)),
        }));
        worker.submit(c(7));
        std::thread::sleep(Duration::from_millis(600));
        let stats = worker.stats();
        assert_eq!(stats.failures, 1);
        assert_eq!(stats.last_error.as_deref(), Some("boom"));
        assert_eq!(lock(&log).as_slice(), &[c(7)]);
    }

    #[test]
    fn drop_stops_worker() {
        let worker = BackendWorker::spawn(Box::new(SlowSink {
            delay: Duration::from_millis(1),
            log: Arc::new(Mutex::new(Vec::new())),
            fail_first: Arc::new(AtomicBool::new(false)),
        }));
        let t = Instant::now();
        drop(worker);
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
