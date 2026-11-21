//! Modern 6-digit temporary PIN pairing & signed Bearer token authentication.

use crate::config::{DaemonConfig, StoredClient};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ctrlmate_protocol::PairingSessionInfo;
use hmac::{Hmac, Mac};
use rand::{Rng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const PAIRING_TTL_SECS: i64 = 120;
const MAX_PIN_ATTEMPTS: u8 = 5;

#[derive(Debug, Clone)]
pub struct ActivePairing {
    pub pin: String,
    pub expires_at: DateTime<Utc>,
    pub failed_attempts: u8,
}

#[derive(Debug, Default)]
pub struct PairingManager {
    session: Option<ActivePairing>,
}

impl PairingManager {
    pub fn start(&mut self) -> ActivePairing {
        let code: u32 = rand::rng().random_range(0..1_000_000);
        let session = ActivePairing {
            pin: format!("{code:06}"),
            expires_at: Utc::now() + Duration::seconds(PAIRING_TTL_SECS),
            failed_attempts: 0,
        };
        self.session = Some(session.clone());
        session
    }

    pub fn cancel(&mut self) {
        self.session = None;
    }

    pub fn status(&mut self, cert_fingerprint: &str, listen_port: u16) -> PairingSessionInfo {
        self.prune_expired();
        match &self.session {
            Some(s) => PairingSessionInfo {
                active: true,
                pin: Some(s.pin.clone()),
                expires_at: Some(s.expires_at),
                cert_fingerprint: cert_fingerprint.to_string(),
                listen_port,
            },
            None => PairingSessionInfo {
                active: false,
                pin: None,
                expires_at: None,
                cert_fingerprint: cert_fingerprint.to_string(),
                listen_port,
            },
        }
    }

    /// Validates a submitted 6-digit PIN in constant time, consuming the pairing session on success.
    pub fn verify_and_consume(&mut self, submitted_pin: &str) -> Result<(), &'static str> {
        self.prune_expired();
        let Some(sess) = self.session.as_mut() else {
            return Err("No active pairing session. Click 'Start Pairing' in Ctrlmate.");
        };

        let normalized = submitted_pin.trim().replace([' ', '-'], "");
        let valid = normalized.len() == sess.pin.len()
            && bool::from(normalized.as_bytes().ct_eq(sess.pin.as_bytes()));

        if valid {
            self.session = None;
            Ok(())
        } else {
            sess.failed_attempts += 1;
            if sess.failed_attempts >= MAX_PIN_ATTEMPTS {
                self.session = None;
                Err("Too many invalid PIN attempts. Pairing session locked.")
            } else {
                Err("Invalid pairing PIN.")
            }
        }
    }

    fn prune_expired(&mut self) {
        if let Some(s) = &self.session {
            if Utc::now() > s.expires_at {
                self.session = None;
            }
        }
    }
}

/// Mints a high-entropy HMAC-signed token and returns `(raw_token, StoredClient)`.
pub fn issue_client_token(cfg: &DaemonConfig, client_name: String) -> (String, StoredClient) {
    let client_id = Uuid::new_v4().to_string();
    let mut nonce = [0u8; 24];
    rand::rng().fill_bytes(&mut nonce);

    let payload = format!("{}.{}", client_id, URL_SAFE_NO_PAD.encode(nonce));
    let secret = hex::decode(&cfg.signing_secret_hex).unwrap_or_default();
    let mut mac = HmacSha256::new_from_slice(&secret).expect("HMAC key accepts any length");
    mac.update(payload.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    let raw_token = format!("cm1_{payload}.{sig}");
    let token_hash = hash_token(&raw_token);

    let stored = StoredClient {
        client_id,
        client_name,
        token_hash,
        paired_at: Utc::now(),
    };

    (raw_token, stored)
}

/// Verifies a presented Bearer token in constant time against persisted clients.
pub fn verify_client_token(cfg: &DaemonConfig, raw_token: &str) -> Option<String> {
    let presented_hash = hash_token(raw_token);
    for client in &cfg.paired_clients {
        if client.token_hash.len() == presented_hash.len()
            && bool::from(
                client
                    .token_hash
                    .as_bytes()
                    .ct_eq(presented_hash.as_bytes()),
            )
        {
            return Some(client.client_id.clone());
        }
    }
    None
}

fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_pairing_and_token_verification_work() {
        let mut cfg = DaemonConfig::default();
        let mut pm = PairingManager::default();
        let sess = pm.start();

        assert!(pm.verify_and_consume("000000").is_err() || sess.pin == "000000");
        if sess.pin != "000000" {
            assert!(pm.verify_and_consume(&sess.pin).is_ok());
            // Session consumed after success
            assert!(pm.verify_and_consume(&sess.pin).is_err());
        }

        let (token, stored) = issue_client_token(&cfg, "Home Assistant".into());
        let expected_id = stored.client_id.clone();
        cfg.paired_clients.push(stored);

        assert_eq!(verify_client_token(&cfg, &token), Some(expected_id));
        assert_eq!(verify_client_token(&cfg, "cm1_bogus"), None);
    }
}
