//! OS updates: Windows Update Agent (COM) and Linux PackageKit (D-Bus through `zbus`).
//!
//! * A scan either succeeds (a real, possibly empty, list) or fails with [`UpdateError`]: a failed scan
//!   is never reported as "0 pending".
//! * Default policy: security and quality updates only. Optional updates, drivers and feature
//!   upgrades are included only when [`UpdatePolicy::include_optional`] is set.
//! * Only one scan / install runs at a time ([`UpdateError::Busy`]).
//! * Reboot detection is live and not sticky (Windows registry; Debian `reboot-required`, Fedora
//!   `needs-restarting -r`, Arch missing running-kernel modules).

use crate::system::os_version_string;
use chrono::Utc;
use ctrlmate_protocol::OsUpdateState;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

/// What to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdatePolicy {
    /// Also report optional updates, drivers and feature upgrades. Default: security + quality only.
    pub include_optional: bool,
}

/// One pending update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateItem {
    pub title: String,
    /// `KB5050009`-style ids (Windows); empty on Linux.
    pub kb_ids: Vec<String>,
    /// Windows Update categories (`Security Updates`, ...) or the PackageKit severity (`security`, ...).
    pub categories: Vec<String>,
}

/// Result of a successful scan or install.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanReport {
    pub state: OsUpdateState,
    pub items: Vec<UpdateItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Another scan or install is running.
    Busy,
    /// No update backend (PackageKit missing, WUA not reachable).
    Unavailable(String),
    /// The backend ran and failed.
    Failed(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("another update operation is running"),
            Self::Unavailable(m) => write!(f, "update backend unavailable: {m}"),
            Self::Failed(m) => write!(f, "update operation failed: {m}"),
        }
    }
}

impl std::error::Error for UpdateError {}

static BUSY: AtomicBool = AtomicBool::new(false);

struct BusyGuard;

impl BusyGuard {
    fn acquire() -> Result<Self, UpdateError> {
        if BUSY.swap(true, Ordering::AcqRel) {
            Err(UpdateError::Busy)
        } else {
            Ok(Self)
        }
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

fn report(items: Vec<UpdateItem>, reboot: bool) -> ScanReport {
    let installed = os_version_string();
    ScanReport {
        state: OsUpdateState {
            installed_version: installed.clone(),
            // There is no "latest version" of an operating system; kept equal to the installed one for
            // wire compatibility. The honest view is `UpdatesInfo` (count + titles + KB ids).
            latest_version: installed,
            pending_count: items.len() as u32,
            pending_titles: items.iter().map(|i| i.title.clone()).collect(),
            in_progress: false,
            reboot_required: reboot,
            last_checked: Some(Utc::now()),
        },
        items,
    }
}

/// Blocking scan (run it on a blocking thread, never on the Tokio executor).
pub fn check_os_updates(policy: UpdatePolicy) -> Result<ScanReport, UpdateError> {
    let _guard = BusyGuard::acquire()?;
    let items = imp::scan(policy).inspect_err(|e| warn!("OS update scan failed: {e}"))?;
    info!("OS update scan complete: {} pending", items.len());
    Ok(report(items, is_reboot_required()))
}

/// Blocking download + install, followed by a fresh scan.
pub fn install_os_updates(policy: UpdatePolicy) -> Result<ScanReport, UpdateError> {
    let _guard = BusyGuard::acquire()?;
    info!("Starting OS update download and installation");
    let reboot = imp::install(policy)?;
    let items = imp::scan(policy)?;
    Ok(report(items, reboot || is_reboot_required()))
}

/// Live reboot-pending detection (cheap on Windows, cached for a minute on Linux).
pub fn is_reboot_required() -> bool {
    imp::reboot_required()
}

// ---------------------------------------------------------------------------------------------
// Pure policy logic (shared, tested on every OS)
// ---------------------------------------------------------------------------------------------

/// Facts about one Windows update needed to apply the policy.
#[derive(Debug, Clone, Default)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) struct WuFacts {
    pub categories: Vec<String>,
    pub is_driver: bool,
    /// `AutoSelectOnWebSites`: the update is *recommended* (not optional).
    pub recommended: bool,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn wu_wanted(f: &WuFacts, policy: UpdatePolicy) -> bool {
    if policy.include_optional {
        return true;
    }
    let cats: Vec<String> = f
        .categories
        .iter()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let has = |n: &str| cats.iter().any(|c| c == n);
    if f.is_driver
        || has("drivers")
        || has("driver sets")
        || has("upgrades")
        || has("feature packs")
    {
        return false;
    }
    if has("security updates")
        || has("critical updates")
        || has("update rollups")
        || has("definition updates")
        || has("service packs")
    {
        return true;
    }
    // Plain "Updates" are quality updates only when recommended; preview / optional ones are not.
    has("updates") && f.recommended
}

/// Splits a PackageKit package id `name;version;arch;data` into `(name, version)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_package_id(id: &str) -> Option<(String, String)> {
    let mut it = id.split(';');
    let name = it.next().filter(|s| !s.is_empty())?;
    let version = it.next().unwrap_or("");
    Some((name.to_string(), version.to_string()))
}

/// Severity label of a PackageKit `Info` enum value, `None` for values that are not updates.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn pk_severity(info: u32) -> Option<&'static str> {
    Some(match info {
        3 => "low",
        4 => "enhancement",
        5 => "normal",
        6 => "bugfix",
        7 => "important",
        8 => "security",
        _ => return None,
    })
}

/// Arch Linux heuristic: the running kernel's module directory vanished (kernel package replaced).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn arch_reboot_needed(running_release: &str, modules_dir_exists: bool) -> bool {
    !running_release.is_empty() && !modules_dir_exists
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{UpdateError, UpdateItem, UpdatePolicy, WuFacts, wu_wanted};
    use ctrlmate_protocol::extract_kb_id;
    use tracing::{info, warn};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::System::UpdateAgent::{
        IUpdate, IUpdateCollection, IUpdateSession, UpdateCollection, UpdateSession,
    };
    use windows::core::BSTR;
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    struct ComScope(bool);
    impl ComScope {
        fn enter() -> Self {
            // SAFETY: balanced with CoUninitialize in Drop when this call succeeded.
            Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }
    impl Drop for ComScope {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: matches the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
        }
    }

    pub fn reboot_required() -> bool {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
        ]
        .iter()
        .any(|k| hklm.open_subkey(k).is_ok())
    }

    fn failed(ctx: &str, e: windows::core::Error) -> UpdateError {
        UpdateError::Failed(format!("{ctx}: {e}"))
    }

    fn session() -> Result<IUpdateSession, UpdateError> {
        // SAFETY: COM was initialised by the caller's ComScope.
        unsafe { CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| UpdateError::Unavailable(format!("Windows Update session: {e}")))
    }

    fn criteria(policy: UpdatePolicy) -> BSTR {
        if policy.include_optional {
            BSTR::from("IsInstalled=0 and IsHidden=0")
        } else {
            BSTR::from("IsInstalled=0 and IsHidden=0 and Type='Software'")
        }
    }

    fn facts_and_item(upd: &IUpdate) -> Option<(WuFacts, UpdateItem)> {
        // SAFETY: COM calls on a live interface pointer.
        unsafe {
            let title = upd.Title().ok()?.to_string();
            let mut categories = Vec::new();
            if let Ok(cats) = upd.Categories() {
                for i in 0..cats.Count().unwrap_or(0) {
                    if let Ok(c) = cats.get_Item(i)
                        && let Ok(n) = c.Name()
                    {
                        categories.push(n.to_string());
                    }
                }
            }
            let mut kb_ids = Vec::new();
            if let Ok(kbs) = upd.KBArticleIDs() {
                for i in 0..kbs.Count().unwrap_or(0) {
                    if let Ok(k) = kbs.get_Item(i) {
                        let k = k.to_string();
                        kb_ids.push(if k.to_ascii_uppercase().starts_with("KB") {
                            k
                        } else {
                            format!("KB{k}")
                        });
                    }
                }
            }
            if kb_ids.is_empty()
                && let Some(kb) = extract_kb_id(&title)
            {
                kb_ids.push(kb);
            }
            // UpdateType: 1 = software, 2 = driver.
            let is_driver = upd.Type().map(|t| t.0 == 2).unwrap_or(false);
            let recommended = upd
                .AutoSelectOnWebSites()
                .map(|b| b.as_bool())
                .unwrap_or(false);
            Some((
                WuFacts {
                    categories: categories.clone(),
                    is_driver,
                    recommended,
                },
                UpdateItem {
                    title,
                    kb_ids,
                    categories,
                },
            ))
        }
    }

    fn search(
        session: &IUpdateSession,
        policy: UpdatePolicy,
    ) -> Result<Vec<(IUpdate, UpdateItem)>, UpdateError> {
        // SAFETY: COM calls on live interface pointers.
        unsafe {
            let searcher = session
                .CreateUpdateSearcher()
                .map_err(|e| failed("CreateUpdateSearcher", e))?;
            let result = searcher
                .Search(&criteria(policy))
                .map_err(|e| failed("Search", e))?;
            let updates = result.Updates().map_err(|e| failed("Updates", e))?;
            let count = updates.Count().map_err(|e| failed("Count", e))?;
            let mut out = Vec::new();
            for i in 0..count {
                let Ok(upd) = updates.get_Item(i) else {
                    continue;
                };
                let Some((facts, item)) = facts_and_item(&upd) else {
                    continue;
                };
                if wu_wanted(&facts, policy) {
                    out.push((upd, item));
                }
            }
            Ok(out)
        }
    }

    pub fn scan(policy: UpdatePolicy) -> Result<Vec<UpdateItem>, UpdateError> {
        let _com = ComScope::enter();
        let s = session()?;
        Ok(search(&s, policy)?.into_iter().map(|(_, i)| i).collect())
    }

    /// Downloads and installs the wanted updates; returns whether the install asked for a reboot.
    pub fn install(policy: UpdatePolicy) -> Result<bool, UpdateError> {
        let _com = ComScope::enter();
        let s = session()?;
        let found = search(&s, policy)?;
        if found.is_empty() {
            return Ok(false);
        }
        // SAFETY: COM calls on live interface pointers.
        unsafe {
            let coll: IUpdateCollection =
                CoCreateInstance(&UpdateCollection, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| failed("UpdateCollection", e))?;
            for (u, item) in &found {
                // Updates that carry an EULA cannot be installed unattended without accepting it.
                if !u.EulaAccepted().map(|b| b.as_bool()).unwrap_or(true) {
                    info!("Accepting the EULA of '{}'", item.title);
                    if let Err(e) = u.AcceptEula() {
                        warn!("AcceptEula failed for '{}': {e}", item.title);
                        continue;
                    }
                }
                let _ = coll.Add(u);
            }
            let downloader = s
                .CreateUpdateDownloader()
                .map_err(|e| failed("CreateUpdateDownloader", e))?;
            downloader
                .SetUpdates(&coll)
                .map_err(|e| failed("SetUpdates", e))?;
            let dl = downloader.Download().map_err(|e| failed("Download", e))?;
            let code = dl.ResultCode().map(|c| c.0).unwrap_or(-1);
            // orcSucceeded = 2, orcSucceededWithErrors = 3
            if !(2..=3).contains(&code) {
                return Err(UpdateError::Failed(format!("download result code {code}")));
            }
            let to_install: IUpdateCollection =
                CoCreateInstance(&UpdateCollection, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| failed("UpdateCollection", e))?;
            let mut ready = 0;
            for (u, _) in &found {
                if u.IsDownloaded().map(|b| b.as_bool()).unwrap_or(false) {
                    let _ = to_install.Add(u);
                    ready += 1;
                }
            }
            if ready == 0 {
                return Err(UpdateError::Failed(
                    "no update was downloaded (administrator rights required)".into(),
                ));
            }
            let installer = s
                .CreateUpdateInstaller()
                .map_err(|e| failed("CreateUpdateInstaller", e))?;
            installer
                .SetUpdates(&to_install)
                .map_err(|e| failed("SetUpdates", e))?;
            let res = installer.Install().map_err(|e| failed("Install", e))?;
            let code = res.ResultCode().map(|c| c.0).unwrap_or(-1);
            let reboot = res.RebootRequired().map(|b| b.as_bool()).unwrap_or(false);
            if !(2..=3).contains(&code) {
                return Err(UpdateError::Failed(format!("install result code {code}")));
            }
            info!("Windows Update install finished: code={code}, reboot_required={reboot}");
            Ok(reboot)
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{UpdateError, UpdateItem, UpdatePolicy, arch_reboot_needed};
    use std::path::Path;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    pub fn scan(policy: UpdatePolicy) -> Result<Vec<UpdateItem>, UpdateError> {
        crate::updates_pk::scan(policy)
    }

    pub fn install(policy: UpdatePolicy) -> Result<bool, UpdateError> {
        crate::updates_pk::install(policy)
    }

    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

    pub fn reboot_required() -> bool {
        let mut c = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, v)) = *c
            && at.elapsed() < Duration::from_secs(60)
        {
            return v;
        }
        let v = detect();
        *c = Some((Instant::now(), v));
        v
    }

    fn detect() -> bool {
        // Debian / Ubuntu
        if Path::new("/var/run/reboot-required").exists()
            || Path::new("/run/reboot-required").exists()
        {
            return true;
        }
        // Fedora / RHEL: `needs-restarting -r` exits 1 when a reboot is needed.
        if let Ok(st) = Command::new("needs-restarting").arg("-r").output()
            && st.status.code() == Some(1)
        {
            return true;
        }
        // Arch: the running kernel's modules were removed by a kernel upgrade.
        if Path::new("/etc/arch-release").exists()
            && let Ok(rel) = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        {
            let rel = rel.trim();
            return arch_reboot_needed(rel, Path::new("/usr/lib/modules").join(rel).exists());
        }
        false
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::{UpdateError, UpdateItem, UpdatePolicy};

    pub fn scan(_: UpdatePolicy) -> Result<Vec<UpdateItem>, UpdateError> {
        Err(UpdateError::Unavailable("unsupported OS".into()))
    }
    pub fn install(_: UpdatePolicy) -> Result<bool, UpdateError> {
        Err(UpdateError::Unavailable("unsupported OS".into()))
    }
    pub fn reboot_required() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(cats: &[&str], driver: bool, rec: bool) -> WuFacts {
        WuFacts {
            categories: cats.iter().map(|s| (*s).to_string()).collect(),
            is_driver: driver,
            recommended: rec,
        }
    }

    #[test]
    fn default_policy_is_security_and_quality_only() {
        let d = UpdatePolicy::default();
        assert!(wu_wanted(
            &f(&["Security Updates", "Windows 11"], false, true),
            d
        ));
        assert!(wu_wanted(&f(&["Critical Updates"], false, true), d));
        assert!(wu_wanted(&f(&["Definition Updates"], false, true), d));
        assert!(
            wu_wanted(&f(&["Updates"], false, true), d),
            "recommended quality update"
        );
        assert!(
            !wu_wanted(&f(&["Updates"], false, false), d),
            "optional preview update"
        );
        assert!(!wu_wanted(&f(&["Drivers"], true, true), d));
        assert!(
            !wu_wanted(&f(&["Upgrades"], false, true), d),
            "feature update"
        );
        assert!(!wu_wanted(&f(&["Feature Packs"], false, true), d));
        assert!(
            !wu_wanted(&f(&[], false, true), d),
            "uncategorised is not assumed to be security"
        );
        // A driver is never security even when mis-labelled.
        assert!(!wu_wanted(&f(&["Security Updates"], true, true), d));
    }

    #[test]
    fn optional_flag_includes_everything() {
        let o = UpdatePolicy {
            include_optional: true,
        };
        assert!(wu_wanted(&f(&["Drivers"], true, false), o));
        assert!(wu_wanted(&f(&["Upgrades"], false, false), o));
    }

    #[test]
    fn packagekit_helpers() {
        assert_eq!(
            parse_package_id("openssl;3.0.2-0ubuntu1.12;amd64;jammy-security"),
            Some(("openssl".into(), "3.0.2-0ubuntu1.12".into()))
        );
        assert_eq!(parse_package_id(""), None);
        assert_eq!(pk_severity(8), Some("security"));
        assert_eq!(pk_severity(1), None, "installed is not an update");
        assert_eq!(
            pk_severity(2),
            None,
            "available (not an update) is not listed"
        );
        assert!(arch_reboot_needed("6.12.1-arch1-1", false));
        assert!(!arch_reboot_needed("6.12.1-arch1-1", true));
        assert!(!arch_reboot_needed("", false));
    }

    #[test]
    fn only_one_operation_at_a_time() {
        let g = BusyGuard::acquire().expect("first");
        assert_eq!(BusyGuard::acquire().err(), Some(UpdateError::Busy));
        drop(g);
        assert!(BusyGuard::acquire().is_ok());
    }

    #[test]
    fn report_counts_and_titles_match() {
        let r = report(
            vec![
                UpdateItem {
                    title: "Cumulative Update (KB5050009)".into(),
                    kb_ids: vec!["KB5050009".into()],
                    categories: vec!["Security Updates".into()],
                },
                UpdateItem::default(),
            ],
            true,
        );
        assert_eq!(r.state.pending_count, 2);
        assert_eq!(r.state.pending_titles.len(), 2);
        assert!(r.state.reboot_required);
        assert!(r.state.last_checked.is_some());
    }
}
