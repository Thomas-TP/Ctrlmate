//! mDNS / Zeroconf announcer so Home Assistant 2026.11 discovers Ctrlmate automatically.

use anyhow::Result;
use ctrlmate_protocol::MDNS_SERVICE_TYPE;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use tracing::{info, warn};

pub struct MdnsAnnouncer {
    _daemon: Option<ServiceDaemon>,
}

impl MdnsAnnouncer {
    pub fn start(
        device_id: &str,
        hostname: &str,
        port: u16,
        sw_version: &str,
        mac: Option<&str>,
        cert_fingerprint: &str,
    ) -> Self {
        match Self::try_register(device_id, hostname, port, sw_version, mac, cert_fingerprint) {
            Ok(daemon) => {
                info!("mDNS Zeroconf broadcasting {MDNS_SERVICE_TYPE} on port {port} as '{hostname}'");
                Self {
                    _daemon: Some(daemon),
                }
            }
            Err(err) => {
                warn!("mDNS Zeroconf announcement disabled ({err}) — manual host entry still works");
                Self { _daemon: None }
            }
        }
    }

    fn try_register(
        device_id: &str,
        hostname: &str,
        port: u16,
        sw_version: &str,
        mac: Option<&str>,
        cert_fingerprint: &str,
    ) -> Result<ServiceDaemon> {
        let mdns = ServiceDaemon::new()?;
        let clean_host = hostname
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>();
        let host_label = if clean_host.is_empty() {
            "ctrlmate-pc".to_string()
        } else {
            clean_host
        };
        let host_fqdn = format!("{host_label}.local.");

        let mut props = HashMap::new();
        props.insert("id".into(), device_id.to_string());
        props.insert("name".into(), hostname.to_string());
        props.insert("ver".into(), sw_version.to_string());
        props.insert(
            "fp".into(),
            cert_fingerprint.chars().take(16).collect::<String>(),
        );
        if let Some(m) = mac {
            props.insert("mac".into(), m.to_string());
        }

        let service_info = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &host_label,
            &host_fqdn,
            "",
            port,
            props,
        )?
        .enable_addr_auto();

        mdns.register(service_info)?;
        Ok(mdns)
    }
}
