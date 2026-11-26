//! Anti-cheat detection: the single source of truth (architecture contract section 7).
//!
//! The process / service tables below are used by
//! * [`crate::conflicts`] (the `*_ac` software conflicts shown in the UI),
//! * [`crate::HardwareCollector`] (auto-pause of the deep-hardware bridge and every SMBus / PawnIO
//!   path, translated into `ctrlmate_rgb::backends::DiscoverOptions` by [`discover_options`]),
//! * the bridge manager (it never starts while [`AntiCheatGate::is_paused`]).
//!
//! While any anti-cheat runs nothing kernel-adjacent is started; after it has been gone for
//! [`RESUME_AFTER`] the gate re-opens by itself. The default mode loads no kernel driver at all.

use ctrlmate_protocol::SoftwareConflict;
use ctrlmate_rgb::backends::DiscoverOptions;
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// An anti-cheat must be gone for this long before the deep-hardware path resumes.
pub const RESUME_AFTER: Duration = Duration::from_secs(30);

/// One known anti-cheat. Process names are compared lower-case, without `.exe`; an entry ending in
/// `*` is a prefix match.
#[derive(Debug)]
pub struct AntiCheat {
    pub id: &'static str,
    pub name: &'static str,
    pub processes: &'static [&'static str],
    /// Windows service names (a service counts only while it is RUNNING).
    pub services: &'static [&'static str],
}

pub const ANTI_CHEATS: &[AntiCheat] = &[
    AntiCheat {
        id: "faceit_ac",
        name: "FACEIT Anti-Cheat",
        processes: &["faceitclient", "faceit", "faceitservice"],
        services: &["FACEIT"],
    },
    AntiCheat {
        id: "vanguard_ac",
        name: "Riot Vanguard",
        processes: &["vgc", "vgtray", "vgk"],
        services: &["vgc", "vgk"],
    },
    AntiCheat {
        id: "eac_ac",
        name: "Easy Anti-Cheat",
        processes: &["easyanticheat*"],
        services: &["EasyAntiCheat", "EasyAntiCheat_EOS"],
    },
    AntiCheat {
        id: "battleye_ac",
        name: "BattlEye",
        processes: &["beservice", "bedaisy", "beclient*"],
        services: &["BEService", "BEDaisy"],
    },
    AntiCheat {
        id: "ricochet_ac",
        name: "Call of Duty Ricochet",
        processes: &["ricochet*"],
        services: &["Ricochet"],
    },
    AntiCheat {
        id: "javelin_ac",
        name: "Javelin Anti-Cheat",
        processes: &["javelin*"],
        services: &["Javelin"],
    },
];

pub const IMPACT: &str =
    "Deep hardware (PawnIO / SMBus / OpenRGB DRAM) is paused while this anti-cheat runs.";

fn normalize(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn process_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

/// Matches process names and running service names against the table. Pure, so it is testable.
pub fn match_anti_cheats<'a>(
    process_names: impl IntoIterator<Item = &'a str>,
    running_services: &[String],
) -> Vec<&'static AntiCheat> {
    let names: Vec<String> = process_names.into_iter().map(normalize).collect();
    ANTI_CHEATS
        .iter()
        .filter(|ac| {
            names
                .iter()
                .any(|n| ac.processes.iter().any(|p| process_matches(p, n)))
                || ac
                    .services
                    .iter()
                    .any(|s| running_services.iter().any(|r| r.eq_ignore_ascii_case(s)))
        })
        .collect()
}

/// Converts a detection to the UI conflict record.
pub fn to_conflict(ac: &AntiCheat) -> SoftwareConflict {
    SoftwareConflict {
        id: ac.id.into(),
        name: ac.name.into(),
        category: "Anti-Cheat".into(),
        running: true,
        impact: IMPACT.into(),
    }
}

/// Translates the gate state into RGB discovery options: while paused, no kernel SMBus and no
/// OpenRGB (which would talk to DRAM through its own SMBus driver).
pub fn discover_options(
    deep_hardware: bool,
    paused: bool,
    base: &DiscoverOptions,
) -> DiscoverOptions {
    DiscoverOptions {
        allow_kernel_smbus: deep_hardware && !paused,
        enable_openrgb: base.enable_openrgb && !paused,
        openrgb_port: base.openrgb_port,
    }
}

/// Process + service scanner. Every [`Self::scan`] re-reads the process list (dead processes
/// are dropped), so a freshly launched anti-cheat is seen within one scan interval.
pub struct AntiCheatScanner {
    sys: System,
}

impl Default for AntiCheatScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl AntiCheatScanner {
    pub fn new() -> Self {
        Self { sys: System::new() }
    }

    /// Currently running anti-cheats.
    pub fn scan(&mut self) -> Vec<&'static AntiCheat> {
        self.scan_with_names().0
    }

    /// Running anti-cheats plus the (fresh) list of every process name, so callers that also look
    /// for vendor software do not walk the process table twice.
    pub fn scan_with_names(&mut self) -> (Vec<&'static AntiCheat>, Vec<String>) {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let names: Vec<String> = self
            .sys
            .processes()
            .values()
            .map(|p| p.name().to_string_lossy().into_owned())
            .collect();
        let services = running_services();
        let hits = match_anti_cheats(names.iter().map(String::as_str), &services);
        (hits, names)
    }
}

/// Names (from the table) of the services that are RUNNING. Windows only.
#[cfg(target_os = "windows")]
pub fn running_services() -> Vec<String> {
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
    };
    use windows::core::PCWSTR;

    let mut out = Vec::new();
    // SAFETY: plain SCM calls with valid, NUL-terminated wide strings; every handle is closed.
    unsafe {
        let Ok(scm) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else {
            return out;
        };
        for ac in ANTI_CHEATS {
            for svc in ac.services {
                let wide: Vec<u16> = svc.encode_utf16().chain(std::iter::once(0)).collect();
                let Ok(h) = OpenServiceW(scm, PCWSTR(wide.as_ptr()), SERVICE_QUERY_STATUS) else {
                    continue;
                };
                let mut st = SERVICE_STATUS::default();
                if QueryServiceStatus(h, &mut st).is_ok() && st.dwCurrentState == SERVICE_RUNNING {
                    out.push((*svc).to_string());
                }
                let _ = CloseServiceHandle(h);
            }
        }
        let _ = CloseServiceHandle(scm);
    }
    out
}

#[cfg(not(target_os = "windows"))]
pub fn running_services() -> Vec<String> {
    Vec::new()
}

/// What changed in the gate on an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateEvent {
    None,
    /// An anti-cheat appeared: stop the bridge / SMBus now.
    Paused,
    /// The anti-cheat has been gone for [`RESUME_AFTER`]: deep hardware may start again.
    Resumed,
}

/// Debounced pause / resume state machine.
#[derive(Debug, Clone, Default)]
pub struct AntiCheatGate {
    paused: bool,
    clear_since: Option<Instant>,
}

impl AntiCheatGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn update(&mut self, now: Instant, anti_cheat_running: bool) -> GateEvent {
        if anti_cheat_running {
            self.clear_since = None;
            if self.paused {
                GateEvent::None
            } else {
                self.paused = true;
                GateEvent::Paused
            }
        } else if self.paused {
            let since = *self.clear_since.get_or_insert(now);
            if now.saturating_duration_since(since) >= RESUME_AFTER {
                self.paused = false;
                self.clear_since = None;
                GateEvent::Resumed
            } else {
                GateEvent::None
            }
        } else {
            GateEvent::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_whole_contract_list() {
        for (proc_name, id) in [
            ("FACEITClient.exe", "faceit_ac"),
            ("faceit.exe", "faceit_ac"),
            ("vgc.exe", "vanguard_ac"),
            ("vgtray.exe", "vanguard_ac"),
            ("EasyAntiCheat.exe", "eac_ac"),
            ("EasyAntiCheat_EOS.exe", "eac_ac"),
            ("EasyAntiCheat_launcher.exe", "eac_ac"),
            ("BEService.exe", "battleye_ac"),
            ("BEDaisy", "battleye_ac"),
            ("ricochet", "ricochet_ac"),
            ("Javelin.exe", "javelin_ac"),
        ] {
            let hits = match_anti_cheats([proc_name], &[]);
            assert!(hits.iter().any(|a| a.id == id), "{proc_name} -> {id}");
        }
    }

    #[test]
    fn services_count_and_ordinary_processes_do_not() {
        let hits = match_anti_cheats(
            ["chrome.exe", "explorer.exe", "vgchide"],
            &["vgk".to_string()],
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "vanguard_ac");
        assert!(match_anti_cheats(["chrome.exe", "vgchide.exe"], &[]).is_empty());
        let hits = match_anti_cheats(
            [],
            &["BEService".to_string(), "EasyAntiCheat_EOS".to_string()],
        );
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn gate_pauses_at_once_and_resumes_after_30s_clear() {
        let t0 = Instant::now();
        let mut g = AntiCheatGate::new();
        assert_eq!(g.update(t0, false), GateEvent::None);
        assert_eq!(g.update(t0, true), GateEvent::Paused);
        assert!(g.is_paused());
        assert_eq!(g.update(t0 + Duration::from_secs(2), true), GateEvent::None);
        // gone, but not for long enough
        assert_eq!(
            g.update(t0 + Duration::from_secs(10), false),
            GateEvent::None
        );
        assert_eq!(
            g.update(t0 + Duration::from_secs(39), false),
            GateEvent::None
        );
        // reappears: the clock restarts
        assert_eq!(
            g.update(t0 + Duration::from_secs(40), true),
            GateEvent::None
        );
        assert_eq!(
            g.update(t0 + Duration::from_secs(41), false),
            GateEvent::None
        );
        assert_eq!(
            g.update(t0 + Duration::from_secs(70), false),
            GateEvent::None
        );
        assert_eq!(
            g.update(t0 + Duration::from_secs(71), false),
            GateEvent::Resumed
        );
        assert!(!g.is_paused());
    }

    #[test]
    fn discover_options_follow_the_gate() {
        let base = DiscoverOptions::default();
        let o = discover_options(true, false, &base);
        assert!(o.allow_kernel_smbus && o.enable_openrgb);
        let o = discover_options(true, true, &base);
        assert!(!o.allow_kernel_smbus && !o.enable_openrgb);
        let o = discover_options(false, false, &base);
        assert!(!o.allow_kernel_smbus && o.enable_openrgb);
    }

    #[test]
    fn scanner_runs_and_sees_a_normal_machine_as_clean_or_not() {
        // Smoke test only: it must not panic and must return a (possibly empty) list.
        let mut s = AntiCheatScanner::new();
        let _ = s.scan();
    }
}
