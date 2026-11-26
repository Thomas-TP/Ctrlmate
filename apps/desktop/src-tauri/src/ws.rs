//! Authenticated `/ipc/ws` connection (architecture contract section 2). The task reconnects with
//! backoff and forwards daemon events to the webview as Tauri events:
//! `daemon://event` (an `IpcEvent`, verbatim JSON) and `daemon://connection` ([`ConnectionStatus`]).

use crate::client::IpcClient;
use crate::error::ErrorCode;
use crate::token;
use ctrlmate_protocol::{IPC_WS_AUTH_TIMEOUT_SECS, IpcAuthFrame};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub const EVENT_DAEMON: &str = "daemon://event";
pub const EVENT_CONNECTION: &str = "daemon://connection";

/// Close code used by the daemon when the IPC token was rotated.
const CLOSE_TOKEN_ROTATED: u16 = 4401;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnState {
    Connecting,
    Connected,
    Unauthorized,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionStatus {
    pub status: ConnState,
    /// Error code explaining `unauthorized` / `offline`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<ErrorCode>,
    /// Consecutive failed attempts.
    pub attempt: u32,
    /// Delay before the next attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_in_ms: Option<u64>,
}

impl ConnectionStatus {
    fn connecting(attempt: u32) -> Self {
        Self {
            status: ConnState::Connecting,
            reason: None,
            attempt,
            retry_in_ms: None,
        }
    }
}

/// Shared state between the connection task and the commands.
pub struct ConnectionHub {
    status: Mutex<ConnectionStatus>,
    wake: Notify,
}

impl ConnectionHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(ConnectionStatus::connecting(0)),
            wake: Notify::new(),
        })
    }

    pub fn status(&self) -> ConnectionStatus {
        self.status
            .lock()
            .map(|g| g.clone())
            .unwrap_or(ConnectionStatus::connecting(0))
    }

    /// Skips the backoff and retries immediately.
    pub fn reconnect_now(&self) {
        self.wake.notify_one();
    }

    fn set(&self, app: &AppHandle, s: ConnectionStatus) {
        if let Ok(mut g) = self.status.lock() {
            *g = s.clone();
        }
        let _ = app.emit(EVENT_CONNECTION, &s);
    }
}

/// Backoff for the n-th consecutive failure: 1, 2, 4, 8, then 15 seconds.
pub fn backoff(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.min(4);
    Duration::from_secs(secs.min(15))
}

enum Failure {
    /// Could not obtain a token (typed reason).
    Token(ErrorCode),
    /// The daemon refused this token.
    Rejected(String),
    /// The daemon rotated its token: re-read immediately.
    Rotated,
    Offline,
}

/// Spawns the long-lived connection task.
pub fn spawn(app: AppHandle, hub: Arc<ConnectionHub>, client: Arc<IpcClient>) {
    tauri::async_runtime::spawn(async move {
        let mut attempt: u32 = 0;
        let mut rejected: HashSet<String> = HashSet::new();
        loop {
            hub.set(&app, ConnectionStatus::connecting(attempt));
            let outcome = connect_once(&app, &hub, &client, &rejected).await;
            let (state, reason, delay) = match outcome {
                Failure::Rotated => {
                    rejected.clear();
                    attempt = 0;
                    (ConnState::Connecting, None, Duration::from_millis(500))
                }
                Failure::Rejected(tok) => {
                    rejected.insert(tok);
                    // Try the next candidate token straight away.
                    attempt = attempt.saturating_add(1);
                    (ConnState::Connecting, None, Duration::from_millis(100))
                }
                Failure::Token(code) => {
                    rejected.clear();
                    attempt = attempt.saturating_add(1);
                    let state = if code == ErrorCode::Unauthorized
                        || code == ErrorCode::TokenPermissionDenied
                        || code == ErrorCode::TokenInvalid
                        || code == ErrorCode::TokenNotFound
                    {
                        ConnState::Unauthorized
                    } else {
                        ConnState::Offline
                    };
                    (state, Some(code), backoff(attempt))
                }
                Failure::Offline => {
                    // A restarted daemon may accept a token it rejected earlier.
                    rejected.clear();
                    attempt = attempt.saturating_add(1);
                    (
                        ConnState::Offline,
                        Some(ErrorCode::DaemonOffline),
                        backoff(attempt),
                    )
                }
            };
            if state != ConnState::Connecting {
                hub.set(
                    &app,
                    ConnectionStatus {
                        status: state,
                        reason,
                        attempt,
                        retry_in_ms: Some(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX)),
                    },
                );
            }
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = hub.wake.notified() => { attempt = 0; }
            }
        }
    });
}

async fn connect_once(
    app: &AppHandle,
    hub: &ConnectionHub,
    client: &IpcClient,
    rejected: &HashSet<String>,
) -> Failure {
    let (source, tok) = match token::resolve(rejected) {
        Ok(v) => v,
        Err(code) => return Failure::Token(code),
    };
    // `ws://` request built from a string: tungstenite adds Host but never an Origin header.
    let Ok(request) = format!("ws://127.0.0.1:{}/ipc/ws", client.port()).into_client_request()
    else {
        return Failure::Offline;
    };
    let connect = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(request),
    )
    .await;
    let Ok(Ok((mut socket, _))) = connect else {
        return Failure::Offline;
    };

    let Ok(frame) = serde_json::to_string(&IpcAuthFrame::Auth { token: tok.clone() }) else {
        return Failure::Offline;
    };
    if socket.send(Message::text(frame)).await.is_err() {
        return Failure::Offline;
    }
    let first = tokio::time::timeout(
        Duration::from_secs(IPC_WS_AUTH_TIMEOUT_SECS + 2),
        socket.next(),
    )
    .await;
    match first {
        Ok(Some(Ok(Message::Text(txt)))) => {
            let ok = serde_json::from_str::<Value>(&txt)
                .ok()
                .and_then(|v| {
                    v.get("type")
                        .and_then(Value::as_str)
                        .map(|t| t == "auth_ok")
                })
                .unwrap_or(false);
            if !ok {
                return Failure::Rejected(tok);
            }
        }
        // Closed (1008 unauthorized) or anything else before auth_ok.
        Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) | Err(_) => {
            return Failure::Rejected(tok);
        }
        Ok(Some(Ok(_))) => return Failure::Rejected(tok),
    }

    client.note_source(source);
    hub.set(
        app,
        ConnectionStatus {
            status: ConnState::Connected,
            reason: None,
            attempt: 0,
            retry_in_ms: None,
        },
    );
    let _ = app.emit(EVENT_DAEMON, serde_json::json!({"type": "auth_ok"}));

    loop {
        match socket.next().await {
            Some(Ok(Message::Text(txt))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                    let _ = app.emit(EVENT_DAEMON, v);
                }
            }
            Some(Ok(Message::Close(frame))) => {
                if frame.is_some_and(|f| u16::from(f.code) == CLOSE_TOKEN_ROTATED) {
                    return Failure::Rotated;
                }
                return Failure::Offline;
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return Failure::Offline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        let secs: Vec<u64> = (0..7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 15, 15, 15]);
    }

    #[test]
    fn status_serializes_as_snake_case() {
        let s = ConnectionStatus {
            status: ConnState::Unauthorized,
            reason: Some(ErrorCode::TokenPermissionDenied),
            attempt: 2,
            retry_in_ms: Some(4000),
        };
        let v = serde_json::to_value(s).unwrap_or_default();
        assert_eq!(v["status"], "unauthorized");
        assert_eq!(v["reason"], "token_permission_denied");
    }
}
