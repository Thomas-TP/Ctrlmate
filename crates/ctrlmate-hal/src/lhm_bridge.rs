//! Optional Windows Deep Hardware Micro-Bridge Manager (`ctrlmate-lhm-bridge` — Option A).
//!
//! - Completely unused on Linux (Linux uses native `/sys/class/hwmon` & `i2c-dev`).
//! - Completely disabled in default `ZeroKernelSafe` (Anti-Cheat Safe) mode.
//! - Automatically pauses & terminates if strict FACEIT Anti-Cheat is detected running.

use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeepSensorSnapshot {
    pub cpu_temp_c: Option<f64>,
    pub cpu_power_w: Option<f64>,
    pub motherboard_name: Option<String>,
    pub motherboard_vrm_temp_c: Option<f64>,
    pub motherboard_chipset_temp_c: Option<f64>,
    pub ram_temp_c: Option<f64>,
    #[serde(default)]
    pub case_fans: Vec<CaseFanReading>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaseFanReading {
    pub id: String,
    pub name: String,
    pub rpm: f64,
    pub duty_pct: Option<f64>,
}

pub struct LhmBridgeManager {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    latest: Arc<Mutex<DeepSensorSnapshot>>,
}

impl LhmBridgeManager {
    pub fn new() -> Self {
        Self {
            child: None,
            stdin: None,
            latest: Arc::new(Mutex::new(DeepSensorSnapshot::default())),
        }
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    pub fn latest(&self) -> DeepSensorSnapshot {
        self.latest
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Starts the optional Windows LibreHardwareMonitor + PawnIO helper binary if present.
    pub fn start(&mut self) -> bool {
        if self.child.is_some() {
            return true;
        }
        #[cfg(not(target_os = "windows"))]
        {
            return false;
        }
        #[cfg(target_os = "windows")]
        {
            let Some(exe) = find_bridge_executable() else {
                warn!("Deep Hardware bridge binary 'ctrlmate-lhm-bridge.exe' not built yet (build crates/ctrlmate-lhm-bridge)");
                return false;
            };

            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;

            match Command::new(&exe)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
            {
                Ok(mut child) => {
                    let stdin = child.stdin.take();
                    if let Some(stdout) = child.stdout.take() {
                        let shared = Arc::clone(&self.latest);
                        std::thread::spawn(move || {
                            let reader = BufReader::new(stdout);
                            for line in reader.lines().map_while(Result::ok) {
                                if let Ok(snap) = serde_json::from_str::<DeepSensorSnapshot>(&line) {
                                    if let Ok(mut guard) = shared.lock() {
                                        *guard = snap;
                                    }
                                }
                            }
                        });
                    }
                    info!("Started optional Windows Deep Hardware bridge: {}", exe.display());
                    self.stdin = stdin;
                    self.child = Some(child);
                    true
                }
                Err(err) => {
                    warn!("Failed spawning LHM Deep Hardware bridge: {err}");
                    false
                }
            }
        }
    }

    /// Sends a PWM fan duty override (`Some(pct)`) or restores BIOS auto (`None`).
    pub fn set_case_fans_duty(&mut self, percent: Option<u8>) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let payload = match percent {
            Some(p) => format!("{{\"cmd\":\"set_all_pwm\",\"percent\":{}}}\n", p.clamp(25, 100)),
            None => "{\"cmd\":\"reset_pwm\"}\n".to_string(),
        };
        let _ = stdin.write_all(payload.as_bytes());
        let _ = stdin.flush();
    }

    /// Cleanly resets motherboard PWM headers to BIOS auto and terminates the helper process.
    pub fn stop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"{\"cmd\":\"reset_pwm\"}\n{\"cmd\":\"quit\"}\n");
            let _ = stdin.flush();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            info!("Stopped optional Windows Deep Hardware bridge (Anti-Cheat Safe / BIOS PWM restored)");
        }
        if let Ok(mut guard) = self.latest.lock() {
            *guard = DeepSensorSnapshot::default();
        }
    }
}

impl Drop for LhmBridgeManager {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "windows")]
fn find_bridge_executable() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("ctrlmate-lhm-bridge.exe"));
        }
    }
    candidates.push(PathBuf::from(
        "crates/ctrlmate-lhm-bridge/bin/Release/net8.0-windows/win-x64/ctrlmate-lhm-bridge.exe",
    ));
    candidates.push(PathBuf::from(
        "crates/ctrlmate-lhm-bridge/bin/Release/net8.0-windows/ctrlmate-lhm-bridge.exe",
    ));
    candidates.into_iter().find(|p| p.exists())
}
