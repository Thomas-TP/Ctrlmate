//! Child-process helper: every child is waited for (no zombies), output is captured up to a limit
//! and a hard timeout kills runaway processes. No shell is ever involved.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Result of [`run_capture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// Exit code; `None` when the process was killed (timeout) or ended by a signal.
    pub code: Option<i32>,
    pub timed_out: bool,
    /// stdout followed by stderr, truncated to the requested size at a character boundary.
    pub output: String,
    pub truncated: bool,
}

impl Captured {
    pub fn success(&self) -> bool {
        !self.timed_out && self.code == Some(0)
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    mut r: R,
    limit: usize,
) -> mpsc::Receiver<(Vec<u8>, bool)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 4096];
        // Keep reading after the limit so the child never blocks on a full pipe.
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            let room = limit.saturating_sub(kept.len());
            if room > 0 {
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
            if n > room {
                truncated = true;
            }
        }
        let _ = tx.send((kept, truncated));
    });
    rx
}

/// Truncates to at most `max` bytes on a UTF-8 character boundary.
pub fn truncate_utf8(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

fn finish(mut child: Child, deadline: Instant) -> (Option<i32>, bool) {
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return (st.code(), false),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait(); // reap: never leave a zombie
                return (None, true);
            }
        }
    }
}

/// Runs `cmd` with stdin closed, waits for it (at most `timeout`) and captures its output.
pub fn run_capture(
    mut cmd: Command,
    timeout: Duration,
    max_output: usize,
) -> std::io::Result<Captured> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    let out_rx = child.stdout.take().map(|s| spawn_reader(s, max_output));
    let err_rx = child.stderr.take().map(|s| spawn_reader(s, max_output));
    let (code, timed_out) = finish(child, Instant::now() + timeout);
    // Readers end when the pipes close; a grandchild that inherited them must not hang us.
    let grab = |rx: Option<mpsc::Receiver<(Vec<u8>, bool)>>| {
        rx.and_then(|r| r.recv_timeout(Duration::from_millis(800)).ok())
            .unwrap_or_default()
    };
    let (mut out, t1) = grab(out_rx);
    let (err, t2) = grab(err_rx);
    out.extend_from_slice(&err);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (output, t3) = truncate_utf8(&text, max_output);
    Ok(Captured {
        code,
        timed_out,
        output,
        truncated: t1 || t2 || t3,
    })
}

/// Convenience for short helper commands (`loginctl`, `pactl`, ...): `Ok(output)` on exit code 0,
/// `Err(NotFound)` when the program is missing, other failures as `Err(Other)`.
pub fn run_ok(program: &str, args: &[&str], timeout: Duration) -> std::io::Result<String> {
    let mut c = Command::new(program);
    c.args(args).env("LC_ALL", "C");
    let r = run_capture(c, timeout, 64 * 1024)?;
    if r.success() {
        Ok(r.output)
    } else {
        Err(std::io::Error::other(format!(
            "{program} failed ({:?}{}): {}",
            r.code,
            if r.timed_out { ", timed out" } else { "" },
            r.output.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", script]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", script]);
            c
        }
    }

    #[test]
    fn captures_output_and_exit_code() {
        let r = run_capture(shell("echo hello"), Duration::from_secs(10), 8192).unwrap();
        assert!(r.success());
        assert!(r.output.contains("hello"));
        let r = run_capture(shell("exit 3"), Duration::from_secs(10), 8192).unwrap();
        assert_eq!(r.code, Some(3));
        assert!(!r.success());
    }

    #[test]
    fn timeout_kills_and_reaps() {
        let script = if cfg!(windows) {
            "ping -n 30 127.0.0.1 >nul"
        } else {
            "sleep 30"
        };
        let t = Instant::now();
        let r = run_capture(shell(script), Duration::from_millis(400), 8192).unwrap();
        assert!(r.timed_out);
        assert_eq!(r.code, None);
        assert!(t.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn output_is_truncated() {
        let script = if cfg!(windows) {
            "for /L %i in (1,1,2000) do @echo 0123456789abcdef"
        } else {
            "i=0; while [ $i -lt 2000 ]; do echo 0123456789abcdef; i=$((i+1)); done"
        };
        let r = run_capture(shell(script), Duration::from_secs(30), 1024).unwrap();
        assert!(r.truncated);
        assert!(r.output.len() <= 1024);
        assert!(r.success());
    }

    #[test]
    fn missing_program_is_not_found() {
        let e = run_capture(
            Command::new("definitely-not-a-program-ctrlmate"),
            Duration::from_secs(2),
            10,
        )
        .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn utf8_truncation_never_splits_a_character() {
        let (s, t) = truncate_utf8("ééé", 3);
        assert_eq!(s, "é");
        assert!(t);
        assert_eq!(truncate_utf8("abc", 10), ("abc".to_string(), false));
    }
}
