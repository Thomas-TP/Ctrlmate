//! Microphone / webcam "in use" detection of the *user session*.
//!
//! * Windows: the per-user `CapabilityAccessManager\ConsentStore` (HKCU, which is the right hive
//!   because this code runs in the user's session): an app is using the device while
//!   `LastUsedTimeStart > 0` and `LastUsedTimeStop == 0`.
//! * Linux microphone: PipeWire / PulseAudio capture streams (`pactl list source-outputs`), ignoring
//!   corked streams, peak-detect meters and monitor sources.
//! * Linux webcam: processes of the current user holding `/dev/video*` open. That needs a `/proc/*/fd`
//!   walk, which is cached for [`WEBCAM_SCAN_TTL`] (never once per second).
//!
//! `None` means "cannot be measured here", not "not in use".

use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivacyReading {
    /// `None`: not measurable.
    pub active: Option<bool>,
    pub apps: Vec<String>,
}

/// Cache lifetime of the (expensive) Linux webcam scan.
pub const WEBCAM_SCAN_TTL: Duration = Duration::from_secs(10);

pub fn microphone() -> PrivacyReading {
    imp::microphone()
}

pub fn webcam() -> PrivacyReading {
    imp::webcam()
}

/// A capture stream as listed by `pactl list source-outputs`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CaptureStream {
    pub app: String,
    pub corked: bool,
    pub media_name: String,
    pub source: String,
}

/// Parses `pactl list source-outputs` (C locale).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_source_outputs(text: &str) -> Vec<CaptureStream> {
    let mut out: Vec<CaptureStream> = Vec::new();
    let unq = |v: &str| v.trim().trim_matches('"').to_string();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("Source Output #") {
            out.push(CaptureStream::default());
        } else if let Some(s) = out.last_mut() {
            if let Some(v) = l.strip_prefix("Corked:") {
                s.corked = v.trim() == "yes";
            } else if let Some(v) = l.strip_prefix("Source:") {
                s.source = v.trim().to_string();
            } else if let Some(v) = l.strip_prefix("application.name =") {
                s.app = unq(v);
            } else if let Some(v) = l.strip_prefix("media.name =") {
                s.media_name = unq(v);
            }
        }
    }
    out
}

/// Which capture streams count as "the microphone is in use". `monitor_sources` are the *indexes*
/// of monitor sources (`pactl list short sources`), because `source-outputs` lists the source index.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn real_capture_apps(
    streams: &[CaptureStream],
    monitor_sources: &[String],
) -> Vec<String> {
    let mut apps: Vec<String> = streams
        .iter()
        .filter(|s| !s.corked)
        .filter(|s| !s.media_name.to_ascii_lowercase().contains("peak detect"))
        .filter(|s| !monitor_sources.contains(&s.source))
        .map(|s| {
            if s.app.is_empty() {
                "unknown".to_string()
            } else {
                s.app.clone()
            }
        })
        .collect();
    apps.sort();
    apps.dedup();
    apps
}

#[cfg(target_os = "windows")]
mod imp {
    use super::PrivacyReading;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    fn consent(capability: &str) -> PrivacyReading {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let path = format!(
            r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{capability}"
        );
        let Ok(base) = hkcu.open_subkey(&path) else {
            // The consent store does not exist (older Windows / policy): not measurable.
            return PrivacyReading::default();
        };
        let mut apps = Vec::new();
        collect(&base, &mut apps);
        if let Ok(np) = base.open_subkey("NonPackaged") {
            collect(&np, &mut apps);
        }
        apps.sort();
        apps.dedup();
        PrivacyReading {
            active: Some(!apps.is_empty()),
            apps,
        }
    }

    fn collect(parent: &RegKey, out: &mut Vec<String>) {
        for name in parent.enum_keys().flatten() {
            if name.eq_ignore_ascii_case("NonPackaged") {
                continue;
            }
            let Ok(sub) = parent.open_subkey(&name) else {
                continue;
            };
            let start: u64 = sub.get_value("LastUsedTimeStart").unwrap_or(0);
            let stop: u64 = sub.get_value("LastUsedTimeStop").unwrap_or(1);
            if start > 0 && stop == 0 {
                out.push(name.rsplit('#').next().unwrap_or(&name).to_string());
            }
        }
    }

    pub fn microphone() -> PrivacyReading {
        consent("microphone")
    }

    pub fn webcam() -> PrivacyReading {
        consent("webcam")
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{PrivacyReading, WEBCAM_SCAN_TTL, parse_source_outputs, real_capture_apps};
    use crate::session::exec::run_ok;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    pub fn microphone() -> PrivacyReading {
        let t = Duration::from_secs(3);
        let Ok(text) = run_ok("pactl", &["list", "source-outputs"], t) else {
            return PrivacyReading::default();
        };
        let monitors: Vec<String> = run_ok("pactl", &["list", "short", "sources"], t)
            .map(|s| {
                s.lines()
                    .filter_map(|l| {
                        let mut p = l.split('\t');
                        let (idx, name) = (p.next()?, p.next()?);
                        name.ends_with(".monitor").then(|| idx.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let apps = real_capture_apps(&parse_source_outputs(&text), &monitors);
        PrivacyReading {
            active: Some(!apps.is_empty()),
            apps,
        }
    }

    static CACHE: Mutex<Option<(Instant, PrivacyReading)>> = Mutex::new(None);

    pub fn webcam() -> PrivacyReading {
        let mut c = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, r)) = c.as_ref()
            && at.elapsed() < WEBCAM_SCAN_TTL
        {
            return r.clone();
        }
        let r = scan_video_users();
        *c = Some((Instant::now(), r.clone()));
        r
    }

    fn scan_video_users() -> PrivacyReading {
        let has_video = std::fs::read_dir("/dev")
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("video"))
            })
            .unwrap_or(false);
        if !has_video {
            return PrivacyReading::default();
        }
        let mut apps = Vec::new();
        let Ok(proc_dir) = std::fs::read_dir("/proc") else {
            return PrivacyReading::default();
        };
        for entry in proc_dir.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            else {
                continue;
            };
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
                continue; // other users' processes: not readable, not ours to see
            };
            for fd in fds.flatten() {
                if std::fs::read_link(fd.path())
                    .is_ok_and(|t| t.to_string_lossy().starts_with("/dev/video"))
                {
                    let comm = std::fs::read_to_string(entry.path().join("comm"))
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|_| pid.to_string());
                    if !apps.contains(&comm) {
                        apps.push(comm);
                    }
                    break;
                }
            }
        }
        PrivacyReading {
            active: Some(!apps.is_empty()),
            apps,
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::PrivacyReading;
    pub fn microphone() -> PrivacyReading {
        PrivacyReading::default()
    }
    pub fn webcam() -> PrivacyReading {
        PrivacyReading::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "Source Output #12\n\tDriver: protocol-native.c\n\tSource: 61\n\tCorked: no\n\tProperties:\n\t\tmedia.name = \"Recording Stream\"\n\t\tapplication.name = \"Firefox\"\nSource Output #13\n\tSource: 61\n\tCorked: yes\n\tProperties:\n\t\tapplication.name = \"Discord\"\nSource Output #14\n\tSource: 61\n\tCorked: no\n\tProperties:\n\t\tmedia.name = \"Peak detect\"\n\t\tapplication.name = \"pavucontrol\"\n";

    #[test]
    fn corked_streams_and_peak_meters_do_not_count() {
        let s = parse_source_outputs(TEXT);
        assert_eq!(s.len(), 3);
        assert!(s[1].corked);
        assert_eq!(real_capture_apps(&s, &[]), vec!["Firefox".to_string()]);
        assert!(real_capture_apps(&s[1..], &[]).is_empty());
        assert!(real_capture_apps(&[], &[]).is_empty());
    }
}
