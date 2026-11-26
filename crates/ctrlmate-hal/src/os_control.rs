//! Machine-wide power actions (shutdown, restart, sleep, hibernate). These do not need the
//! interactive session, so the daemon runs them itself. Lock / display-off / notifications / audio /
//! scripts live in [`crate::session`].

use crate::session::exec::run_capture;
use anyhow::{Context, Result, bail};
use std::process::Command;
use std::time::Duration;
use tracing::info;

fn run_waited(program: &str, args: &[&str]) -> Result<()> {
    let mut c = Command::new(program);
    c.args(args);
    let r = run_capture(c, Duration::from_secs(20), 2048)
        .with_context(|| format!("failed to run {program}"))?;
    if r.success() {
        Ok(())
    } else {
        bail!("{program} failed ({:?}): {}", r.code, r.output.trim())
    }
}

/// Executes `power_shutdown`, `power_restart`, `power_sleep` or `power_hibernate`.
pub fn execute_power_action(action_id: &str) -> Result<String> {
    info!("Executing OS power action: {action_id}");

    #[cfg(target_os = "windows")]
    {
        match action_id {
            "power_shutdown" => {
                run_waited(
                    "shutdown",
                    &["/s", "/t", "3", "/c", "Ctrlmate: shutdown requested"],
                )?;
                Ok("shutdown scheduled".into())
            }
            "power_restart" => {
                run_waited(
                    "shutdown",
                    &["/r", "/t", "3", "/c", "Ctrlmate: restart requested"],
                )?;
                Ok("restart scheduled".into())
            }
            "power_sleep" | "power_hibernate" => {
                let hibernate = action_id == "power_hibernate";
                // SAFETY: plain call; the return value says whether the state change was accepted.
                let ok = unsafe {
                    windows::Win32::System::Power::SetSuspendState(hibernate, false, false)
                };
                if ok.as_bool() {
                    Ok(if hibernate { "hibernating" } else { "sleeping" }.into())
                } else {
                    bail!("SetSuspendState was refused by the system")
                }
            }
            _ => bail!("unknown power action: {action_id}"),
        }
    }

    #[cfg(target_os = "linux")]
    {
        let verb = match action_id {
            "power_shutdown" => "poweroff",
            "power_restart" => "reboot",
            "power_sleep" => "suspend",
            "power_hibernate" => "hibernate",
            _ => bail!("unknown power action: {action_id}"),
        };
        run_waited("systemctl", &[verb])?;
        Ok(format!("{verb} requested"))
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = run_waited;
        bail!("unsupported OS for power action: {action_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_session_actions_are_refused() {
        // Lock and display-off moved to the session module; they must never run from here.
        for id in ["power_lock", "display_off", "nonsense", ""] {
            assert!(execute_power_action(id).is_err(), "{id}");
        }
    }
}
