//! Automatic local TLS 1.3 TOFU (Trust-On-First-Use) certificate generation & loading.

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DnType, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

pub struct TlsMaterial {
    pub server_config: Arc<ServerConfig>,
    /// Colon-free lowercase hex SHA-256 fingerprint of the DER certificate.
    pub cert_fingerprint: String,
}

pub fn load_or_generate_tls(data_dir: &Path, hostname: &str) -> Result<TlsMaterial> {
    let cert_path = data_dir.join("tls-cert.der");
    let key_path = data_dir.join("tls-key.der");

    let (cert_der, key_der) = if cert_path.exists() && key_path.exists() {
        let cert = std::fs::read(&cert_path).context("failed reading TLS cert")?;
        let key = std::fs::read(&key_path).context("failed reading TLS key")?;
        (cert, key)
    } else {
        info!("Generating new local TLS 1.3 TOFU certificate for '{hostname}'");
        let mut params = CertificateParams::new(vec![
            "localhost".into(),
            hostname.to_string(),
            format!("{hostname}.local"),
        ])?;
        params
            .distinguished_name
            .push(DnType::CommonName, format!("Ctrlmate ({hostname})"));
        params
            .distinguished_name
            .push(DnType::OrganizationName, "Ctrlmate Local");
        params
            .subject_alt_names
            .push(SanType::IpAddress(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)));

        let key_pair = KeyPair::generate()?;
        let cert = params.self_signed(&key_pair)?;
        let cert_der = cert.der().to_vec();
        let key_der = key_pair.serialize_der();

        std::fs::write(&cert_path, &cert_der)?;
        std::fs::write(&key_path, &key_der)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }

        (cert_der, key_der)
    };

    let mut hasher = Sha256::new();
    hasher.update(&cert_der);
    let cert_fingerprint = hex::encode(hasher.finalize());

    let certs = vec![CertificateDer::from(cert_der)];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));

    let provider = rustls::crypto::ring::default_provider();
    let mut config = ServerConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("failed building rustls ServerConfig")?;

    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    Ok(TlsMaterial {
        server_config: Arc::new(config),
        cert_fingerprint,
    })
}
