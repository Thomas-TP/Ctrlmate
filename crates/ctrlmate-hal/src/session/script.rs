//! Allowlisted custom scripts (architecture contract section 3): absolute executable, argument
//! vector (no shell), 60 s timeout, output truncated to 8 KiB, child always waited for.

use super::exec::run_capture;
use ctrlmate_protocol::{AllowlistedScript, CommandResult, ResultCode};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tracing::info;

pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);
pub const SCRIPT_MAX_OUTPUT: usize = 8 * 1024;

/// Runs `script` and maps the outcome to a [`CommandResult`]:
/// * `ok` with the captured output on exit code 0,
/// * `invalid_argument` for a disabled script or a bad path,
/// * `unavailable` when the executable does not exist,
/// * `hardware_error` for a non-zero exit, a timeout, or a spawn failure (technical detail in `message`).
pub fn run_script(script: &AllowlistedScript) -> CommandResult {
    run_script_with(script, SCRIPT_TIMEOUT, SCRIPT_MAX_OUTPUT)
}

pub(crate) fn run_script_with(
    script: &AllowlistedScript,
    timeout: Duration,
    max_output: usize,
) -> CommandResult {
    if !script.enabled {
        return CommandResult::err_with(ResultCode::InvalidArgument, "script disabled");
    }
    let exe = Path::new(&script.executable);
    if !exe.is_absolute() {
        return CommandResult::err_with(ResultCode::InvalidArgument, "executable must be absolute");
    }
    if !exe.is_file() {
        return CommandResult::err_with(ResultCode::Unavailable, "executable not found");
    }
    if let Some(dir) = &script.working_dir
        && !Path::new(dir).is_dir()
    {
        return CommandResult::err_with(ResultCode::InvalidArgument, "working directory not found");
    }
    let mut cmd = Command::new(exe);
    cmd.args(&script.args);
    if let Some(dir) = &script.working_dir {
        cmd.current_dir(dir);
    }
    info!("Running allowlisted script '{}'", script.id);
    match run_capture(cmd, timeout, max_output) {
        Ok(r) if r.success() => CommandResult::ok_with(r.output),
        Ok(r) if r.timed_out => CommandResult::err_with(
            ResultCode::HardwareError,
            format!(
                "timed out after {} s; output: {}",
                timeout.as_secs(),
                r.output
            ),
        ),
        Ok(r) => CommandResult::err_with(
            ResultCode::HardwareError,
            format!("exit code {:?}; output: {}", r.code, r.output),
        ),
        Err(e) => CommandResult::err_with(ResultCode::HardwareError, format!("spawn failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(exe: &str, args: &[&str]) -> AllowlistedScript {
        AllowlistedScript {
            id: "t".into(),
            name: "t".into(),
            executable: exe.into(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            working_dir: None,
            enabled: true,
        }
    }

    fn sys_exe() -> String {
        if cfg!(windows) {
            std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into())
        } else {
            "/bin/sh".into()
        }
    }

    fn sh_args(script: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["/C".into(), script.into()]
        } else {
            vec!["-c".into(), script.into()]
        }
    }

    #[test]
    fn runs_with_args_vector_and_returns_output() {
        let a = sh_args("echo from-script");
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let r = run_script(&script(&sys_exe(), &a));
        assert!(r.ok, "{r:?}");
        assert!(r.message.unwrap().contains("from-script"));
    }

    #[test]
    fn rejects_relative_missing_and_disabled() {
        assert_eq!(run_script(&script("cmd.exe", &[])).code, "invalid_argument");
        let missing = if cfg!(windows) {
            r"C:\nope\nothing.exe"
        } else {
            "/nope/nothing"
        };
        assert_eq!(run_script(&script(missing, &[])).code, "unavailable");
        let mut s = script(&sys_exe(), &[]);
        s.enabled = false;
        assert_eq!(run_script(&s).code, "invalid_argument");
        let mut s = script(&sys_exe(), &[]);
        s.working_dir = Some(if cfg!(windows) {
            r"C:\nope-dir".into()
        } else {
            "/nope-dir".into()
        });
        assert_eq!(run_script(&s).code, "invalid_argument");
    }

    #[test]
    fn nonzero_exit_and_timeout_are_errors_with_detail() {
        let a = sh_args("echo boom & exit 7");
        let a2 = sh_args("echo boom; exit 7");
        let args = if cfg!(windows) { a } else { a2 };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let r = run_script(&script(&sys_exe(), &args));
        assert_eq!(r.code, "hardware_error");
        let m = r.message.unwrap();
        assert!(m.contains('7') && m.contains("boom"), "{m}");

        let slow = sh_args(if cfg!(windows) {
            "ping -n 30 127.0.0.1 >nul"
        } else {
            "sleep 30"
        });
        let slow: Vec<&str> = slow.iter().map(String::as_str).collect();
        let r = run_script_with(&script(&sys_exe(), &slow), Duration::from_millis(400), 1024);
        assert_eq!(r.code, "hardware_error");
        assert!(r.message.unwrap().contains("timed out"));
    }

    #[test]
    fn output_is_capped_at_8_kib() {
        let body = if cfg!(windows) {
            "for /L %i in (1,1,3000) do @echo 0123456789abcdef"
        } else {
            "i=0; while [ $i -lt 3000 ]; do echo 0123456789abcdef; i=$((i+1)); done"
        };
        let a = sh_args(body);
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let r = run_script(&script(&sys_exe(), &a));
        assert!(r.ok);
        assert!(r.message.unwrap().len() <= SCRIPT_MAX_OUTPUT);
    }
}
