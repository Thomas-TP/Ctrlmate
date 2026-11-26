//! Minimal `sd_notify` client (Linux): `READY=1` after the listeners are bound and `STOPPING=1`
//! on shutdown, so `Type=notify` units know when the daemon is really up. No-op elsewhere or
//! when `NOTIFY_SOCKET` is not set.

/// Sends one datagram to `$NOTIFY_SOCKET`. Returns whether it was delivered.
#[cfg(target_os = "linux")]
pub fn sd_notify(message: &str) -> bool {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else {
        return false;
    };
    let raw = path.to_string_lossy().to_string();
    let Ok(sock) = UnixDatagram::unbound() else {
        return false;
    };
    let addr = if let Some(name) = raw.strip_prefix('@') {
        SocketAddr::from_abstract_name(name.as_bytes())
    } else {
        SocketAddr::from_pathname(&raw)
    };
    let Ok(addr) = addr else {
        return false;
    };
    sock.send_to_addr(message.as_bytes(), &addr).is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn sd_notify(_message: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn notify_without_socket_is_a_noop() {
        // Never panics, and reports "not delivered" when systemd is not around.
        if std::env::var_os("NOTIFY_SOCKET").is_none() {
            assert!(!super::sd_notify("READY=1"));
        }
    }
}
