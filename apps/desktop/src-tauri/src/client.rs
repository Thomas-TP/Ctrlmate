//! Allowlisted REST client for `/ipc/*` (architecture contract section 2). The webview calls
//! `ipc_request`; the bearer token is attached here and never returned.

use crate::error::{ErrorCode, IpcError};
use crate::token;
use reqwest::Method;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct IpcResponse {
    pub status: u16,
    pub body: Value,
}

/// Validates `method` + `path` against the allowlist of the IPC surface.
pub fn validate_route(method: &str, path: &str) -> Result<Method, IpcError> {
    if path.contains('?') || path.contains('#') || path.contains("..") || path.contains('\\') {
        return Err(ErrorCode::RouteNotAllowed.into());
    }
    let m = method.to_ascii_uppercase();
    let ok = match (m.as_str(), path) {
        ("GET", "/ipc/snapshot")
        | ("POST", "/ipc/pairing/start" | "/ipc/pairing/cancel")
        | ("POST", "/ipc/command")
        | ("PUT", "/ipc/settings")
        | ("POST", "/ipc/hardware/rescan") => true,
        ("DELETE", p) => p.strip_prefix("/ipc/clients/").is_some_and(valid_client_id),
        _ => false,
    };
    if !ok {
        return Err(ErrorCode::RouteNotAllowed.into());
    }
    Method::from_bytes(m.as_bytes()).map_err(|_| ErrorCode::RouteNotAllowed.into())
}

fn valid_client_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

pub struct IpcClient {
    http: reqwest::Client,
    port: u16,
    last_source: Mutex<Option<token::TokenSource>>,
}

impl IpcClient {
    pub fn new(port: u16) -> Result<Self, IpcError> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(3))
            .build()
            .map_err(|e| IpcError::with_detail(ErrorCode::Internal, e.to_string()))?;
        Ok(Self {
            http,
            port,
            last_source: Mutex::new(None),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn last_source(&self) -> Option<token::TokenSource> {
        self.last_source.lock().ok().and_then(|g| *g)
    }

    pub fn note_source(&self, source: token::TokenSource) {
        if let Ok(mut g) = self.last_source.lock() {
            *g = Some(source);
        }
    }

    /// Sends one request. A 401 makes the client try the next token source before giving up.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<IpcResponse, IpcError> {
        let m = validate_route(method, path)?;
        let timeout = if path == "/ipc/command" {
            Duration::from_secs(90) // custom scripts may run for 60 s
        } else {
            Duration::from_secs(15)
        };
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let mut rejected: HashSet<String> = HashSet::new();
        loop {
            let (source, tok) = token::resolve(&rejected).map_err(IpcError::new)?;
            let mut req = self
                .http
                .request(m.clone(), &url)
                .timeout(timeout)
                .bearer_auth(&tok);
            if let Some(b) = &body {
                let text = serde_json::to_string(b)
                    .map_err(|e| IpcError::with_detail(ErrorCode::BadRequest, e.to_string()))?;
                req = req.header("content-type", "application/json").body(text);
            }
            let resp = req.send().await.map_err(|e| {
                if e.is_connect() || e.is_timeout() || e.is_request() {
                    IpcError::new(ErrorCode::DaemonOffline)
                } else {
                    IpcError::with_detail(ErrorCode::Internal, e.to_string())
                }
            })?;
            let status = resp.status().as_u16();
            if status == 401 {
                rejected.insert(tok);
                continue;
            }
            self.note_source(source);
            let bytes = resp
                .bytes()
                .await
                .map_err(|_| IpcError::new(ErrorCode::DaemonOffline))?;
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap_or(Value::Null)
            };
            return Ok(IpcResponse { status, body });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_accepts_only_the_known_routes() {
        for (m, p) in [
            ("GET", "/ipc/snapshot"),
            ("post", "/ipc/pairing/start"),
            ("POST", "/ipc/pairing/cancel"),
            ("POST", "/ipc/command"),
            ("PUT", "/ipc/settings"),
            ("POST", "/ipc/hardware/rescan"),
            ("DELETE", "/ipc/clients/3f2a-91bc_x.y"),
        ] {
            assert!(validate_route(m, p).is_ok(), "{m} {p}");
        }
        for (m, p) in [
            ("GET", "/ipc/ws"),
            ("GET", "/ipc/agent"),
            ("GET", "/ipc/snapshot?x=1"),
            ("POST", "/ipc/snapshot"),
            ("DELETE", "/ipc/clients/"),
            ("DELETE", "/ipc/clients/../snapshot"),
            ("DELETE", "/ipc/clients/a/b"),
            ("GET", "http://evil.example/ipc/snapshot"),
            ("GET", "/api/v1/info"),
            ("PATCH", "/ipc/settings"),
        ] {
            assert_eq!(
                validate_route(m, p).map_err(|e| e.code),
                Err(ErrorCode::RouteNotAllowed),
                "{m} {p}"
            );
        }
    }
}
