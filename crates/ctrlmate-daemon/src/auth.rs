//! PIN pairing with per-IP throttling, and persistent client bearer tokens.
//!
//! Token design: `cm2_` + 192 random bits (hex). Only `SHA-256(token)` is stored. With 192 bits
//! of entropy no KDF or HMAC is needed; an HMAC that is never verified would be theatre, so the
//! old signature part is gone. Legacy `cm1_...` tokens keep working because verification is
//! just "hash the presented string and compare".
//!
//! PIN throttling: a stranger on the LAN must not be able to lock the real user out, so failures
//! are tracked per source IP. Each IP gets [`MAX_FAILURES_PER_IP`] wrong guesses per pairing
//! session and an exponential backoff between guesses. The session itself is only cancelled by a
//! very high global failure ceiling ([`GLOBAL_FAILURE_CEILING`]), which a single host cannot reach.

use crate::config::StoredClient;
use chrono::{DateTime, Duration, Utc};
use ctrlmate_protocol::PairingSessionInfo;
use rand::{Rng, RngExt};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration as StdDuration, Instant};
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const PAIRING_TTL_SECS: i64 = 120;
pub const MAX_FAILURES_PER_IP: u32 = 5;
pub const GLOBAL_FAILURE_CEILING: u32 = 100;
pub const MAX_CLIENT_NAME_CHARS: usize = 64;
const MAX_TRACKED_IPS: usize = 1024;
const IP_FORGET_AFTER: StdDuration = StdDuration::from_secs(600);
const MAX_BACKOFF_SECS: u64 = 300;
pub const DEFAULT_CLIENT_NAME: &str = "Home Assistant";

/// Why a pairing attempt was refused. `code()` is the stable machine string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    NoSession,
    InvalidPin { retry_after_secs: u64 },
    Throttled { retry_after_secs: u64 },
}

impl PairError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoSession => "no_session",
            Self::InvalidPin { .. } => "invalid_pin",
            Self::Throttled { .. } => "throttled",
        }
    }

    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::NoSession => None,
            Self::InvalidPin { retry_after_secs } | Self::Throttled { retry_after_secs } => {
                Some(*retry_after_secs)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivePairing {
    pub pin: String,
    pub expires_at: DateTime<Utc>,
    global_failures: u32,
}

#[derive(Debug, Clone)]
struct IpBudget {
    session_failures: u32,
    streak: u32,
    blocked_until: Option<Instant>,
    last_seen: Instant,
}

#[derive(Debug, Default)]
pub struct PairingManager {
    session: Option<ActivePairing>,
    ips: HashMap<IpAddr, IpBudget>,
}

fn backoff(streak: u32) -> StdDuration {
    let exp = streak.saturating_sub(1).min(16);
    StdDuration::from_secs((1u64 << exp).min(MAX_BACKOFF_SECS))
}

fn ceil_secs(d: StdDuration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

impl PairingManager {
    pub fn start(&mut self) -> ActivePairing {
        self.start_at(Utc::now())
    }

    fn start_at(&mut self, now: DateTime<Utc>) -> ActivePairing {
        let code: u32 = rand::rng().random_range(0..1_000_000);
        let session = ActivePairing {
            pin: format!("{code:06}"),
            expires_at: now + Duration::seconds(PAIRING_TTL_SECS),
            global_failures: 0,
        };
        // Per-session budgets restart; the backoff streak deliberately survives.
        for b in self.ips.values_mut() {
            b.session_failures = 0;
            b.blocked_until = None;
        }
        self.session = Some(session.clone());
        session
    }

    pub fn cancel(&mut self) {
        self.session = None;
    }

    pub fn status(&mut self, cert_fingerprint: &str, listen_port: u16) -> PairingSessionInfo {
        self.prune_expired(Utc::now());
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

    pub fn is_active(&mut self) -> bool {
        self.prune_expired(Utc::now());
        self.session.is_some()
    }

    /// Validates a submitted PIN from `ip` in constant time, consuming the session on success.
    pub fn verify_and_consume(&mut self, ip: IpAddr, submitted_pin: &str) -> Result<(), PairError> {
        self.verify_at(ip, submitted_pin, Instant::now(), Utc::now())
    }

    fn verify_at(
        &mut self,
        ip: IpAddr,
        submitted_pin: &str,
        now: Instant,
        wall: DateTime<Utc>,
    ) -> Result<(), PairError> {
        self.prune_expired(wall);
        self.prune_ips(now);
        let Some(sess) = self.session.as_mut() else {
            return Err(PairError::NoSession);
        };

        let budget = self.ips.entry(ip).or_insert(IpBudget {
            session_failures: 0,
            streak: 0,
            blocked_until: None,
            last_seen: now,
        });
        budget.last_seen = now;
        if let Some(until) = budget.blocked_until
            && until > now
        {
            return Err(PairError::Throttled {
                retry_after_secs: ceil_secs(until - now).max(1),
            });
        }

        let normalized = submitted_pin.trim().replace([' ', '-'], "");
        let pin_ok = normalized.len() == sess.pin.len()
            && bool::from(normalized.as_bytes().ct_eq(sess.pin.as_bytes()));

        if pin_ok {
            budget.streak = 0;
            budget.session_failures = 0;
            budget.blocked_until = None;
            self.session = None;
            return Ok(());
        }

        budget.session_failures += 1;
        budget.streak += 1;
        sess.global_failures += 1;
        let delay = if budget.session_failures >= MAX_FAILURES_PER_IP {
            // Out of guesses for this session: shut this IP out until the PIN expires.
            let remaining = (sess.expires_at - wall).to_std().unwrap_or_default();
            remaining.max(backoff(budget.streak))
        } else {
            backoff(budget.streak)
        };
        budget.blocked_until = Some(now + delay);
        let retry_after_secs = ceil_secs(delay).max(1);

        if sess.global_failures >= GLOBAL_FAILURE_CEILING {
            tracing::warn!("Pairing PIN cancelled: global failure ceiling reached");
            self.session = None;
        }
        Err(PairError::InvalidPin { retry_after_secs })
    }

    fn prune_expired(&mut self, wall: DateTime<Utc>) {
        if let Some(s) = &self.session
            && wall > s.expires_at
        {
            self.session = None;
        }
    }

    fn prune_ips(&mut self, now: Instant) {
        if self.ips.len() < MAX_TRACKED_IPS {
            return;
        }
        self.ips
            .retain(|_, b| now.saturating_duration_since(b.last_seen) < IP_FORGET_AFTER);
        while self.ips.len() >= MAX_TRACKED_IPS {
            let Some(oldest) = self
                .ips
                .iter()
                .min_by_key(|(_, b)| b.last_seen)
                .map(|(ip, _)| *ip)
            else {
                break;
            };
            self.ips.remove(&oldest);
        }
    }
}

/// Strips control characters, collapses whitespace, trims and limits the length (in chars).
pub fn sanitize_client_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let limited: String = collapsed.chars().take(MAX_CLIENT_NAME_CHARS).collect();
    let limited = limited.trim().to_string();
    if limited.is_empty() {
        DEFAULT_CLIENT_NAME.to_string()
    } else {
        limited
    }
}

/// Hex SHA-256 of a token string.
pub fn hash_token(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}

/// Mints a client token and returns `(raw_token, StoredClient)`.
pub fn issue_client_token(client_name: &str) -> (String, StoredClient) {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    let raw_token = format!("cm2_{}", hex::encode(bytes));
    let stored = StoredClient {
        client_id: Uuid::new_v4().to_string(),
        client_name: sanitize_client_name(client_name),
        token_hash: hash_token(&raw_token),
        paired_at: Utc::now(),
    };
    (raw_token, stored)
}

/// Finds the client owning `raw_token`; every stored hash is compared in constant time.
pub fn verify_client_token(clients: &[StoredClient], raw_token: &str) -> Option<String> {
    let presented = hash_token(raw_token);
    let mut found = None;
    for c in clients {
        let eq = c.token_hash.len() == presented.len()
            && bool::from(c.token_hash.as_bytes().ct_eq(presented.as_bytes()));
        if eq {
            found = Some(c.client_id.clone());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, n))
    }

    fn wrong_pin(real: &str) -> &'static str {
        if real == "000000" { "111111" } else { "000000" }
    }

    #[test]
    fn correct_pin_pairs_and_consumes_session() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        assert!(
            pm.verify_and_consume(ip(1), &format!(" {}-{} ", &s.pin[..3], &s.pin[3..]))
                .is_ok()
        );
        assert_eq!(
            pm.verify_and_consume(ip(1), &s.pin),
            Err(PairError::NoSession)
        );
    }

    #[test]
    fn stranger_cannot_lock_out_the_real_user() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        let t0 = Instant::now();
        let w0 = Utc::now();
        // The attacker burns its whole budget (waiting out the backoff each time).
        let mut t = t0;
        let mut last_fail = t0;
        for _ in 0..MAX_FAILURES_PER_IP {
            last_fail = t;
            match pm.verify_at(ip(66), wrong_pin(&s.pin), t, w0) {
                Err(PairError::InvalidPin { retry_after_secs }) => {
                    t += StdDuration::from_secs(retry_after_secs);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        // Now the attacker is locked for the rest of the session, even with the right PIN.
        assert!(matches!(
            pm.verify_at(ip(66), &s.pin, last_fail + StdDuration::from_secs(30), w0),
            Err(PairError::Throttled { .. })
        ));
        // The real user, from another IP, still pairs.
        assert!(pm.verify_at(ip(2), &s.pin, last_fail, w0).is_ok());
    }

    #[test]
    fn backoff_grows_exponentially_per_ip() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        let t0 = Instant::now();
        let w0 = Utc::now();
        let bad = wrong_pin(&s.pin);
        let mut waits = vec![];
        let mut t = t0;
        for _ in 0..4 {
            let Err(PairError::InvalidPin { retry_after_secs }) = pm.verify_at(ip(9), bad, t, w0)
            else {
                panic!("expected invalid pin");
            };
            waits.push(retry_after_secs);
            // Trying again immediately is throttled, not counted.
            assert!(matches!(
                pm.verify_at(ip(9), bad, t, w0),
                Err(PairError::Throttled { .. })
            ));
            t += StdDuration::from_secs(retry_after_secs);
        }
        assert_eq!(waits, vec![1, 2, 4, 8]);
    }

    #[test]
    fn throttled_attempts_do_not_burn_the_global_budget() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        let t0 = Instant::now();
        let w0 = Utc::now();
        let _ = pm.verify_at(ip(5), wrong_pin(&s.pin), t0, w0);
        for _ in 0..500 {
            let _ = pm.verify_at(ip(5), wrong_pin(&s.pin), t0, w0);
        }
        assert!(pm.session.is_some());
        assert_eq!(pm.session.as_ref().map(|s| s.global_failures), Some(1));
    }

    #[test]
    fn global_ceiling_needs_many_hosts() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        let t0 = Instant::now();
        let w0 = Utc::now();
        let bad = wrong_pin(&s.pin);
        for n in 0..(GLOBAL_FAILURE_CEILING as usize) {
            // 25 hosts x 4 failures each: below the per-IP limit, reaches the ceiling.
            let host = ip((n % 25) as u8 + 10);
            let mut t = t0;
            // Skip backoff by advancing time per attempt.
            t += StdDuration::from_secs(1000 * (n as u64 / 25));
            let _ = pm.verify_at(host, bad, t, w0);
        }
        assert!(pm.session.is_none(), "ceiling should cancel the session");
    }

    #[test]
    fn new_session_resets_session_budget_not_streak() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        let t0 = Instant::now();
        let w0 = Utc::now();
        let _ = pm.verify_at(ip(7), wrong_pin(&s.pin), t0, w0);
        let s2 = pm.start_at(w0);
        // Immediately usable again (block cleared) with the new PIN.
        assert!(pm.verify_at(ip(7), &s2.pin, t0, w0).is_ok());
    }

    #[test]
    fn expired_session_is_gone() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        let later = Utc::now() + Duration::seconds(PAIRING_TTL_SECS + 5);
        assert_eq!(
            pm.verify_at(ip(1), &s.pin, Instant::now(), later),
            Err(PairError::NoSession)
        );
    }

    #[test]
    fn ip_table_is_bounded() {
        let mut pm = PairingManager::default();
        let s = pm.start();
        let t0 = Instant::now();
        let w0 = Utc::now();
        for i in 0..(MAX_TRACKED_IPS as u32 + 200) {
            let host = IpAddr::V4(Ipv4Addr::from(0x0A00_0000 + i));
            let _ = pm.verify_at(
                host,
                wrong_pin(&s.pin),
                t0 + StdDuration::from_millis(u64::from(i)),
                w0,
            );
        }
        assert!(pm.ips.len() <= MAX_TRACKED_IPS);
    }

    #[test]
    fn client_names_are_sanitised() {
        assert_eq!(
            sanitize_client_name("  Home\u{0}\nAssistant\t"),
            "Home Assistant"
        );
        assert_eq!(sanitize_client_name(""), DEFAULT_CLIENT_NAME);
        assert_eq!(sanitize_client_name("\u{7}\u{7}"), DEFAULT_CLIENT_NAME);
        let long = "x".repeat(500);
        assert_eq!(
            sanitize_client_name(&long).chars().count(),
            MAX_CLIENT_NAME_CHARS
        );
        let multibyte = "é".repeat(100);
        assert_eq!(
            sanitize_client_name(&multibyte).chars().count(),
            MAX_CLIENT_NAME_CHARS
        );
    }

    #[test]
    fn tokens_hash_only_and_verify() {
        let (tok, stored) = issue_client_token("HA");
        assert!(tok.starts_with("cm2_"));
        assert_eq!(tok.len(), 4 + 48);
        assert!(!serde_json::to_string(&stored).unwrap().contains(&tok));
        let clients = vec![stored.clone()];
        assert_eq!(verify_client_token(&clients, &tok), Some(stored.client_id));
        assert_eq!(verify_client_token(&clients, "cm2_bogus"), None);
        assert_eq!(verify_client_token(&[], &tok), None);
    }

    #[test]
    fn legacy_tokens_still_verify() {
        let legacy = "cm1_abc.def";
        let c = StoredClient {
            client_id: "c1".into(),
            client_name: "old".into(),
            token_hash: hash_token(legacy),
            paired_at: Utc::now(),
        };
        assert_eq!(verify_client_token(&[c], legacy), Some("c1".into()));
    }
}
