//! Single-instance lock: a second agent of the same user session exits at once.
//!
//! Windows: a named mutex in the `Local\` namespace (per session). Linux: an exclusive `flock` on a
//! file in `$XDG_RUNTIME_DIR` (or the temp dir). The lock is released when the guard (or the process)
//! goes away, so a crash never leaves a stale lock.

use anyhow::Result;

/// Holds the lock while alive.
pub struct InstanceLock {
    #[cfg(target_os = "windows")]
    _handle: HandleGuard,
    #[cfg(target_os = "linux")]
    _file: std::fs::File,
}

#[cfg(target_os = "windows")]
struct HandleGuard(windows::Win32::Foundation::HANDLE);

// SAFETY: a mutex HANDLE is a plain kernel object id; it is only closed in Drop.
#[cfg(target_os = "windows")]
unsafe impl Send for HandleGuard {}

#[cfg(target_os = "windows")]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateMutexW.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

impl InstanceLock {
    /// `Ok(Some(lock))` when this is the only instance, `Ok(None)` when another one holds `name`.
    pub fn acquire(name: &str) -> Result<Option<Self>> {
        imp::acquire(name)
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{HandleGuard, InstanceLock};
    use anyhow::{Result, anyhow};
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;

    pub fn acquire(name: &str) -> Result<Option<InstanceLock>> {
        let wide: Vec<u16> = format!("Local\\{name}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: valid NUL-terminated name; GetLastError is read right after the call.
        unsafe {
            let h = CreateMutexW(None, true, PCWSTR(wide.as_ptr()))
                .map_err(|e| anyhow!("CreateMutexW: {e}"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = windows::Win32::Foundation::CloseHandle(h);
                return Ok(None);
            }
            Ok(Some(InstanceLock {
                _handle: HandleGuard(h),
            }))
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::InstanceLock;
    use anyhow::{Context, Result};
    use std::os::fd::AsRawFd;

    pub fn acquire(name: &str) -> Result<Option<InstanceLock>> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        // The uid in the file name keeps two users on a shared temp dir apart.
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let path = dir.join(format!("{name}-{uid}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        // SAFETY: valid open fd; LOCK_NB makes the call non-blocking.
        let r = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if r == 0 {
            Ok(Some(InstanceLock { _file: file }))
        } else {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(e).context("flock failed")
            }
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::InstanceLock;
    use anyhow::Result;
    pub fn acquire(_: &str) -> Result<Option<InstanceLock>> {
        Ok(Some(InstanceLock {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_instance_is_refused_until_the_first_ends() {
        let name = format!("ctrlmate-agent-test-{}", std::process::id());
        let first = InstanceLock::acquire(&name)
            .unwrap()
            .expect("first instance");
        assert!(
            InstanceLock::acquire(&name).unwrap().is_none(),
            "second must be refused"
        );
        drop(first);
        assert!(
            InstanceLock::acquire(&name).unwrap().is_some(),
            "free again after the first ended"
        );
    }
}
