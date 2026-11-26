//! Self-signed TOFU certificate + a TLS **1.3-only** server configuration.

use crate::datadir::{DataDir, Protection, read_optional};
use anyhow::{Context, Result};
use rcgen::{CertificateParams, DnType, KeyPair, SanType};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::sync::Arc;
use tracing::info;

const CERT_FILE: &str = "tls-cert.der";
const KEY_FILE: &str = "tls-key.der";

pub struct TlsMaterial {
    pub server_config: Arc<ServerConfig>,
    /// Lowercase hex SHA-256 of the DER certificate (64 characters, never truncated).
    pub cert_fingerprint: String,
}

pub fn fingerprint_hex(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

/// Builds the server config. TLS 1.2 and below are not offered at all.
pub fn build_server_config(cert_der: Vec<u8>, key_der: Vec<u8>) -> Result<ServerConfig> {
    let certs = vec![CertificateDer::from(cert_der)];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
    let provider = rustls::crypto::ring::default_provider();
    let mut config = ServerConfig::builder_with_provider(provider.into())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3 is not available in the crypto provider")?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("failed building rustls ServerConfig")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

pub fn load_or_generate_tls(dd: &DataDir, hostname: &str) -> Result<TlsMaterial> {
    let cert_path = dd.file(CERT_FILE);
    let key_path = dd.file(KEY_FILE);

    let existing = match (read_optional(&cert_path)?, read_optional(&key_path)?) {
        (Some(c), Some(k)) => Some((c, k)),
        _ => None,
    };
    let (cert_der, key_der) = if let Some(pair) = existing {
        // Files written by older versions were never ACL-protected.
        let _ = dd.protect_existing(KEY_FILE, Protection::Secret);
        let _ = dd.protect_existing(CERT_FILE, Protection::Secret);
        pair
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
        params.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(
            std::net::Ipv4Addr::LOCALHOST,
        )));

        let key_pair = KeyPair::generate()?;
        let cert = params.self_signed(&key_pair)?;
        let cert_der = cert.der().to_vec();
        let key_der = key_pair.serialize_der();
        // Key first: if we crash in between, the next start regenerates both.
        dd.write_protected(KEY_FILE, &key_der, Protection::Secret)?;
        dd.write_protected(CERT_FILE, &cert_der, Protection::Secret)?;
        (cert_der, key_der)
    };

    let cert_fingerprint = fingerprint_hex(&cert_der);
    let server_config = Arc::new(build_server_config(cert_der, key_der)?);
    Ok(TlsMaterial {
        server_config,
        cert_fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctrlmate_protocol::DataDirKind;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    fn dd(p: &std::path::Path) -> DataDir {
        DataDir {
            path: p.to_path_buf(),
            kind: DataDirKind::Custom,
        }
    }

    #[test]
    fn fingerprint_is_full_64_hex_and_stable_across_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        d.prepare().unwrap();
        let a = load_or_generate_tls(&d, "my-pc").unwrap();
        let b = load_or_generate_tls(&d, "my-pc").unwrap();
        assert_eq!(a.cert_fingerprint.len(), 64);
        assert!(a.cert_fingerprint.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a.cert_fingerprint, b.cert_fingerprint);
    }

    /// A client verifier that accepts any certificate (pinning is not under test here).
    #[derive(Debug)]
    struct AcceptAll;
    impl rustls::client::danger::ServerCertVerifier for AcceptAll {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                m,
                c,
                d,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                m,
                c,
                d,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    async fn handshake(
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> Result<String, String> {
        let tmp = tempfile::tempdir().unwrap();
        let d = dd(tmp.path());
        d.prepare().unwrap();
        let tls = load_or_generate_tls(&d, "my-pc").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(tls.server_config);
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await
                && let Ok(mut s) = acceptor.accept(tcp).await
            {
                let _ = s.write_all(b"hi").await;
                let _ = s.shutdown().await;
            }
        });
        let client = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_protocol_versions(versions)
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAll))
        .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(client));
        let tcp = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| e.to_string())?;
        let mut s = connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .map_err(|e| e.to_string())?;
        let version = format!("{:?}", s.get_ref().1.protocol_version());
        let mut buf = [0u8; 2];
        let _ = s.read_exact(&mut buf).await;
        Ok(version)
    }

    #[tokio::test]
    async fn server_negotiates_tls13_and_refuses_tls12() {
        let ok = handshake(&[&rustls::version::TLS13]).await.unwrap();
        assert!(ok.contains("TLSv1_3"), "{ok}");
        let tls12 = handshake(&[&rustls::version::TLS12]).await;
        assert!(tls12.is_err(), "a TLS 1.2-only client must be refused");
    }
}
