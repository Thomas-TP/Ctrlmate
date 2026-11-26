//! mDNS / Zeroconf announcer so Home Assistant discovers Ctrlmate automatically.
//!
//! TXT records: `id` (device id), `name` (host name, already part of the mDNS host label),
//! `ver` (daemon version), `fp` (the **full** 64-hex certificate SHA-256, used by Home Assistant
//! to pin the certificate). The MAC address is deliberately not broadcast on the LAN; Home
//! Assistant receives it after pairing.

use anyhow::Result;
use ctrlmate_protocol::MDNS_SERVICE_TYPE;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use tracing::{info, warn};

pub struct MdnsAnnouncer {
    daemon: Option<ServiceDaemon>,
}

/// TXT properties of the service announcement.
pub fn txt_properties(
    device_id: &str,
    hostname: &str,
    sw_version: &str,
    cert_fingerprint: &str,
) -> HashMap<String, String> {
    let mut props = HashMap::new();
    props.insert("id".into(), device_id.to_string());
    props.insert("name".into(), hostname.to_string());
    props.insert("ver".into(), sw_version.to_string());
    props.insert("fp".into(), cert_fingerprint.to_string());
    props
}

fn host_label(hostname: &str) -> String {
    let clean: String = hostname
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if clean.is_empty() {
        "ctrlmate-pc".to_string()
    } else {
        clean
    }
}

impl MdnsAnnouncer {
    pub fn start(
        device_id: &str,
        hostname: &str,
        port: u16,
        sw_version: &str,
        cert_fingerprint: &str,
    ) -> Self {
        match Self::try_register(device_id, hostname, port, sw_version, cert_fingerprint) {
            Ok(daemon) => {
                info!("mDNS broadcasting {MDNS_SERVICE_TYPE} on port {port} as '{hostname}'");
                Self {
                    daemon: Some(daemon),
                }
            }
            Err(err) => {
                warn!("mDNS announcement disabled ({err}); manual host entry still works");
                Self { daemon: None }
            }
        }
    }

    fn try_register(
        device_id: &str,
        hostname: &str,
        port: u16,
        sw_version: &str,
        cert_fingerprint: &str,
    ) -> Result<ServiceDaemon> {
        let mdns = ServiceDaemon::new()?;
        let label = host_label(hostname);
        let host_fqdn = format!("{label}.local.");
        let props = txt_properties(device_id, hostname, sw_version, cert_fingerprint);
        let service_info =
            ServiceInfo::new(MDNS_SERVICE_TYPE, &label, &host_fqdn, "", port, props)?
                .enable_addr_auto();
        mdns.register(service_info)?;
        Ok(mdns)
    }

    /// Withdraws the announcement (goodbye packet) on shutdown.
    pub fn shutdown(self) {
        if let Some(d) = self.daemon {
            let _ = d.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_carries_the_full_fingerprint_and_no_mac() {
        let fp = "ab".repeat(32);
        let p = txt_properties("dev", "PC", "0.6.0", &fp);
        assert_eq!(p["fp"].len(), 64);
        assert_eq!(p["fp"], fp);
        assert!(!p.contains_key("mac"));
        // Each TXT entry (key=value) must stay under the 255 byte DNS limit.
        assert!(p.iter().all(|(k, v)| k.len() + 1 + v.len() <= 255));
    }

    #[test]
    fn host_labels_are_dns_safe() {
        assert_eq!(host_label("My PC_01"), "MyPC01");
        assert_eq!(host_label("!!!"), "ctrlmate-pc");
    }
}
