//! PackageKit over the system D-Bus (`org.freedesktop.PackageKit`), Linux only.
//!
//! Flow of every operation: `CreateTransaction`, subscribe to the transaction's signals, call the
//! method, then collect `Package` / `ErrorCode` / `RequireRestart` until `Finished`. Signals are read on
//! a helper thread so the whole operation has a hard timeout.
//!
//! PackageKit enum values used here (from `pk-enum.h`): `Info` 3 low, 4 enhancement, 5 normal, 6 bugfix,
//! 7 important, 8 security; `Exit` 1 success, 2 failed, 3 cancelled, 4 key required, 5 EULA required,
//! 8 untrusted; `Restart` 4 system, 6 security-system; filter bitfield `1 << 1` = none;
//! transaction-flag bitfield `1 << 1` = only trusted.

use crate::updates::{UpdateError, UpdateItem, UpdatePolicy, parse_package_id, pk_severity};
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

const DEST: &str = "org.freedesktop.PackageKit";
const SCAN_TIMEOUT: Duration = Duration::from_secs(300);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(3600);
const FILTER_NONE: u64 = 1 << 1;
const TRANSACTION_ONLY_TRUSTED: u64 = 1 << 1;

#[derive(Debug)]
enum Sig {
    Package(u32, String),
    Error(u32, String),
    Restart(u32),
    Finished(u32),
}

struct Outcome {
    packages: Vec<(u32, String)>,
    restart: bool,
    exit: u32,
    error: Option<String>,
}

fn unavailable(e: impl std::fmt::Display) -> UpdateError {
    UpdateError::Unavailable(format!("PackageKit: {e}"))
}

fn exit_name(code: u32) -> &'static str {
    match code {
        2 => "failed",
        3 => "cancelled",
        4 => "key_required",
        5 => "eula_required",
        6 => "killed",
        8 => "need_untrusted",
        _ => "unknown",
    }
}

fn run(
    call: impl FnOnce(&Proxy<'_>) -> zbus::Result<()>,
    timeout: Duration,
) -> Result<Outcome, UpdateError> {
    let conn = Connection::system().map_err(unavailable)?;
    let pk = Proxy::new(
        &conn,
        DEST,
        "/org/freedesktop/PackageKit",
        "org.freedesktop.PackageKit",
    )
    .map_err(unavailable)?;
    let path: OwnedObjectPath = pk.call("CreateTransaction", &()).map_err(unavailable)?;
    let tx = Proxy::new(
        &conn,
        DEST,
        path.as_ref(),
        "org.freedesktop.PackageKit.Transaction",
    )
    .map_err(unavailable)?;
    let signals = tx.receive_all_signals().map_err(unavailable)?;
    let (stx, srx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ctrlmate-pk-signals".into())
        .spawn(move || {
            for msg in signals {
                let hdr = msg.header();
                let member = hdr
                    .member()
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                let body = msg.body();
                let sig = match member.as_str() {
                    "Package" => body
                        .deserialize::<(u32, String, String)>()
                        .ok()
                        .map(|(info, id, _summary)| Sig::Package(info, id)),
                    "ErrorCode" => body
                        .deserialize::<(u32, String)>()
                        .ok()
                        .map(|(c, d)| Sig::Error(c, d)),
                    "RequireRestart" => body
                        .deserialize::<(u32, String)>()
                        .ok()
                        .map(|(t, _)| Sig::Restart(t)),
                    "Finished" => body
                        .deserialize::<(u32, u32)>()
                        .ok()
                        .map(|(exit, _)| Sig::Finished(exit)),
                    _ => None,
                };
                if let Some(s) = sig {
                    let done = matches!(s, Sig::Finished(_));
                    if stx.send(s).is_err() || done {
                        break;
                    }
                }
            }
        })
        .map_err(unavailable)?;
    call(&tx).map_err(|e| UpdateError::Failed(format!("PackageKit call: {e}")))?;

    let mut out = Outcome {
        packages: Vec::new(),
        restart: false,
        exit: 0,
        error: None,
    };
    loop {
        match srx.recv_timeout(timeout) {
            Ok(Sig::Package(info, id)) => out.packages.push((info, id)),
            Ok(Sig::Error(code, details)) => out.error = Some(format!("code {code}: {details}")),
            Ok(Sig::Restart(t)) => out.restart |= t >= 4,
            Ok(Sig::Finished(exit)) => {
                out.exit = exit;
                return Ok(out);
            }
            Err(_) => return Err(UpdateError::Failed("PackageKit timed out".into())),
        }
    }
}

fn check_exit(o: &Outcome) -> Result<(), UpdateError> {
    if o.exit == 1 {
        Ok(())
    } else {
        Err(UpdateError::Failed(format!(
            "{}{}",
            exit_name(o.exit),
            o.error
                .as_deref()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default()
        )))
    }
}

fn list_updates(policy: UpdatePolicy) -> Result<Vec<(UpdateItem, String)>, UpdateError> {
    // pkcon refresh
    let refresh = run(
        |tx| tx.call::<_, _, ()>("RefreshCache", &(false,)),
        SCAN_TIMEOUT,
    )?;
    check_exit(&refresh)?;
    let o = run(
        |tx| tx.call::<_, _, ()>("GetUpdates", &(FILTER_NONE,)),
        SCAN_TIMEOUT,
    )?;
    check_exit(&o)?;
    let mut items = Vec::new();
    for (info, id) in o.packages {
        let Some(sev) = pk_severity(info) else {
            continue;
        };
        // Default: security + quality. Enhancements and "low" are optional.
        if !policy.include_optional && matches!(sev, "low" | "enhancement") {
            continue;
        }
        let Some((name, version)) = parse_package_id(&id) else {
            continue;
        };
        items.push((
            UpdateItem {
                title: format!("{name} {version}").trim().to_string(),
                kb_ids: Vec::new(),
                categories: vec![sev.to_string()],
            },
            id,
        ));
    }
    Ok(items)
}

pub fn scan(policy: UpdatePolicy) -> Result<Vec<UpdateItem>, UpdateError> {
    Ok(list_updates(policy)?.into_iter().map(|(i, _)| i).collect())
}

/// Installs the wanted updates; returns whether PackageKit asked for a restart.
pub fn install(policy: UpdatePolicy) -> Result<bool, UpdateError> {
    let ids: Vec<String> = list_updates(policy)?
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    if ids.is_empty() {
        return Ok(false);
    }
    let o = run(
        |tx| tx.call::<_, _, ()>("UpdatePackages", &(TRANSACTION_ONLY_TRUSTED, ids)),
        INSTALL_TIMEOUT,
    )?;
    check_exit(&o)?;
    Ok(o.restart)
}
