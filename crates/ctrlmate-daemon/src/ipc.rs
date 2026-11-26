//! Loopback IPC (`127.0.0.1:9488`) for the desktop app and the agent.
//!
//! Security (architecture contract section 2): constant-time bearer token on every route, except
//! the two WebSockets which authenticate with a first text frame within 5 s; any `Origin` header
//! is rejected (403); `Host` must be `127.0.0.1:<port>` or `localhost:<port>` (anti DNS
//! rebinding, 421); there is deliberately no CORS layer.

use crate::sessions::AgentHello;
use crate::state::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use ctrlmate_protocol::{
    AgentMessage, CommandResult, ControlCommand, DaemonToAgent, IpcAuthFrame, IpcEvent, ResultCode,
    SettingsUpdate,
};
use std::sync::Arc;
use tracing::{debug, info, warn};

const MAX_IPC_MESSAGE: usize = 256 * 1024;
const MAX_IPC_BODY: usize = 256 * 1024;

pub fn build_ipc_router(state: Arc<AppState>) -> Router {
    let authed = Router::new()
        .route("/ipc/snapshot", get(ipc_snapshot))
        .route("/ipc/pairing/start", post(ipc_start_pairing))
        .route("/ipc/pairing/cancel", post(ipc_cancel_pairing))
        .route("/ipc/clients/{client_id}", delete(ipc_revoke_client))
        .route("/ipc/command", post(ipc_command))
        .route("/ipc/settings", put(ipc_update_settings))
        .route("/ipc/hardware/rescan", post(ipc_rescan_hardware))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_bearer,
        ));
    // The WebSockets authenticate with their first frame instead of a header.
    let sockets = Router::new()
        .route("/ipc/ws", get(ipc_ws_upgrade))
        .route("/ipc/agent", get(ipc_agent_upgrade));
    authed
        .merge(sockets)
        .layer(DefaultBodyLimit::max(MAX_IPC_BODY))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            guard_origin_and_host,
        ))
        .with_state(state)
}

fn reject(status: StatusCode, code: ResultCode) -> Response {
    (status, Json(CommandResult::err(code))).into_response()
}

fn host_allowed(host: &str, port: u16) -> bool {
    let host = host.trim().to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

/// Rejects browsers (`Origin`) and rebinding attacks (`Host`) before anything else runs.
async fn guard_origin_and_host(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if req.headers().contains_key(header::ORIGIN) {
        return reject(StatusCode::FORBIDDEN, ResultCode::PermissionDenied);
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_string()));
    match host {
        Some(h) if host_allowed(&h, state.ipc_port) => next.run(req).await,
        _ => reject(StatusCode::MISDIRECTED_REQUEST, ResultCode::InvalidArgument),
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

async fn require_bearer(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    match bearer(req.headers()) {
        Some(t) if state.ipc_token.check(t) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(CommandResult::err(ResultCode::PermissionDenied)),
        )
            .into_response(),
    }
}

// --- REST handlers ---------------------------------------------------------------------------

async fn ipc_snapshot(State(state): State<Arc<AppState>>) -> Response {
    Json(state.snapshot().await).into_response()
}

async fn ipc_start_pairing(State(state): State<Arc<AppState>>) -> Response {
    let expires = crate::state::lock(&state.pairing).start().expires_at;
    info!("Started HA pairing session (expires at {expires})");
    state.notify_ui().await;
    Json(state.snapshot().await).into_response()
}

async fn ipc_cancel_pairing(State(state): State<Arc<AppState>>) -> Response {
    crate::state::lock(&state.pairing).cancel();
    state.notify_ui().await;
    Json(state.snapshot().await).into_response()
}

async fn ipc_revoke_client(
    State(state): State<Arc<AppState>>,
    AxumPath(client_id): AxumPath<String>,
) -> Response {
    if !state.revoke_client(&client_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(CommandResult::err_with(
                ResultCode::InvalidArgument,
                "unknown client",
            )),
        )
            .into_response();
    }
    Json(state.snapshot().await).into_response()
}

async fn ipc_command(
    State(state): State<Arc<AppState>>,
    Json(cmd): Json<ControlCommand>,
) -> Response {
    Json(state.execute_command(cmd, false).await).into_response()
}

async fn ipc_update_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettingsUpdate>,
) -> Response {
    let result = state.apply_settings(req).await;
    if result.ok {
        Json(state.snapshot().await).into_response()
    } else {
        let status = if result.result_code() == ResultCode::InvalidArgument {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, Json(result)).into_response()
    }
}

async fn ipc_rescan_hardware(State(state): State<Arc<AppState>>) -> Response {
    if state.hw.rescan().await.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(CommandResult::err(ResultCode::Unavailable)),
        )
            .into_response();
    }
    state.refresh_manifest().await;
    Json(state.snapshot().await).into_response()
}

// --- WebSockets ------------------------------------------------------------------------------

/// First-frame authentication: `{"type":"auth","token":"..."}` within the timeout.
async fn authenticate_ws(socket: &mut WebSocket, state: &AppState) -> bool {
    let first = tokio::time::timeout(state.ws_auth_timeout, socket.recv()).await;
    let ok = match first {
        Ok(Some(Ok(Message::Text(txt)))) => match serde_json::from_str::<IpcAuthFrame>(&txt) {
            Ok(IpcAuthFrame::Auth { token }) => state.ipc_token.check(&token),
            Err(_) => false,
        },
        _ => false,
    };
    if !ok {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "unauthorized".into(),
            })))
            .await;
    }
    ok
}

async fn send_json<T: serde::Serialize>(socket: &mut WebSocket, v: &T) -> bool {
    match serde_json::to_string(v) {
        Ok(txt) => socket.send(Message::Text(txt.into())).await.is_ok(),
        Err(_) => false,
    }
}

async fn ipc_ws_upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.max_message_size(MAX_IPC_MESSAGE)
        .on_upgrade(move |socket| ui_session(socket, state))
}

async fn ui_session(mut socket: WebSocket, state: Arc<AppState>) {
    if !authenticate_ws(&mut socket, &state).await {
        return;
    }
    let mut rotation = state.ipc_token.subscribe();
    rotation.borrow_and_update();
    let mut shutdown = state.shutdown.subscribe();
    let mut rx = state.ui_tx.subscribe();
    state.sampler_wake.notify_one();

    if !send_json(&mut socket, &IpcEvent::AuthOk).await {
        return;
    }
    let initial = state.snapshot().await;
    if !send_json(&mut socket, &IpcEvent::Snapshot(Box::new(initial))).await {
        return;
    }
    loop {
        tokio::select! {
            _ = rotation.changed() => {
                let _ = socket.send(Message::Close(Some(CloseFrame { code: 4401, reason: "token rotated".into() }))).await;
                break;
            }
            _ = shutdown.changed() => break,
            snap = rx.recv() => {
                let event = match snap {
                    Ok(s) => IpcEvent::Snapshot(Box::new((*s).clone())),
                    // Missed snapshots: send a fresh one, not a stale queue.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        IpcEvent::Snapshot(Box::new(state.snapshot().await))
                    }
                    Err(_) => break,
                };
                if !send_json(&mut socket, &event).await { break; }
            }
            msg = socket.recv() => {
                if matches!(msg, None | Some(Ok(Message::Close(_)) | Err(_))) { break; }
            }
        }
    }
}

async fn ipc_agent_upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.max_message_size(MAX_IPC_MESSAGE)
        .on_upgrade(move |socket| agent_session(socket, state))
}

async fn agent_session(mut socket: WebSocket, state: Arc<AppState>) {
    if !authenticate_ws(&mut socket, &state).await {
        return;
    }
    let backend = Arc::clone(&state.sessions.agent);
    let mut att = backend.attach();
    let conn_id = att.conn_id;
    info!("Agent connected [conn {conn_id}]");

    let mut rotation = state.ipc_token.subscribe();
    rotation.borrow_and_update();
    let mut shutdown = state.shutdown.subscribe();

    let greeted = send_json(&mut socket, &DaemonToAgent::AuthOk).await
        && send_json(
            &mut socket,
            &DaemonToAgent::Scripts {
                scripts: backend.scripts(),
            },
        )
        .await;
    if greeted {
        // Agent presence changes agent_connected, the in-process fallback and the tier.
        state.on_session_changed().await;
        // Scripts configured before the agent connected.
        let scripts = state.config.read().await.scripts.clone();
        backend.push_scripts(&scripts);

        loop {
            tokio::select! {
                _ = rotation.changed() => {
                    let _ = socket.send(Message::Close(Some(CloseFrame { code: 4401, reason: "token rotated".into() }))).await;
                    break;
                }
                _ = shutdown.changed() => break,
                _ = att.kicked.changed() => {
                    debug!("Agent connection {conn_id} replaced by a newer agent");
                    let _ = socket.send(Message::Close(Some(CloseFrame { code: 4409, reason: "replaced".into() }))).await;
                    break;
                }
                Some(frame) = att.outgoing.recv() => {
                    if !send_json(&mut socket, &frame).await { break; }
                }
                msg = socket.recv() => match msg {
                    Some(Ok(Message::Text(txt))) => match serde_json::from_str::<AgentMessage>(&txt) {
                        Ok(AgentMessage::Hello { agent_version, os, session_id, user }) => {
                            info!(
                                "Agent hello: v{agent_version} on {os}, session {session_id}, user '{}'",
                                user.chars().take(128).collect::<String>()
                            );
                            backend.set_hello(conn_id, AgentHello { agent_version, session_id });
                        }
                        Ok(AgentMessage::SessionState(s)) => {
                            if backend.set_state(conn_id, s) {
                                state.on_session_changed().await;
                            }
                        }
                        Ok(AgentMessage::Response { id, ok, code, message }) => {
                            backend.resolve(conn_id, &id, CommandResult { ok, code, message });
                        }
                        Err(e) => warn!("Ignoring malformed agent frame: {e}"),
                    },
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
            }
        }
    }
    backend.detach(conn_id);
    info!("Agent disconnected [conn {conn_id}]");
    state.on_session_changed().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionBackend;
    use crate::state::testutil::{RigOpts, TestRig, rig, rig_opts};
    use axum::body::Body;
    use axum::http::Request;
    use ctrlmate_protocol::{SessionAction, SessionState};
    use futures_util::{SinkExt, StreamExt};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as WsMsg;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tower::ServiceExt;

    const HOST: &str = "127.0.0.1:9488";

    fn req(method: &str, uri: &str) -> axum::http::request::Builder {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("host", HOST)
    }

    async fn send(rig: &TestRig, r: Request<Body>) -> (StatusCode, serde_json::Value) {
        let resp = build_ipc_router(Arc::clone(&rig.state))
            .oneshot(r)
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn authed(rig: &TestRig, method: &str, uri: &str) -> axum::http::request::Builder {
        req(method, uri).header("authorization", format!("Bearer {}", rig.token))
    }

    // --- bearer ----------------------------------------------------------------------------

    #[tokio::test]
    async fn every_rest_route_requires_the_token() {
        let rig = rig().await;
        let routes = [
            ("GET", "/ipc/snapshot"),
            ("POST", "/ipc/pairing/start"),
            ("POST", "/ipc/pairing/cancel"),
            ("DELETE", "/ipc/clients/abc"),
            ("POST", "/ipc/command"),
            ("PUT", "/ipc/settings"),
            ("POST", "/ipc/hardware/rescan"),
        ];
        for (m, uri) in routes {
            // Missing token.
            let (status, body) = send(&rig, req(m, uri).body(Body::empty()).unwrap()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{m} {uri} without token");
            assert_eq!(body["code"], "permission_denied");
            // Wrong token.
            let (status, _) = send(
                &rig,
                req(m, uri)
                    .header("authorization", format!("Bearer {}", "0".repeat(64)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{m} {uri} wrong token");
            // Wrong scheme / token prefix / empty.
            for bad in [
                format!("Basic {}", rig.token),
                rig.token.clone(),
                "Bearer ".into(),
                "Bearer".into(),
            ] {
                let (status, _) = send(
                    &rig,
                    req(m, uri)
                        .header("authorization", bad)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{m} {uri}");
            }
        }
        // And nothing was executed by the rejected calls.
        assert!(rig.state.config.read().await.paired_clients.is_empty());
        assert!(!rig.state.snapshot().await.pairing.active);
    }

    #[tokio::test]
    async fn valid_token_reaches_the_handlers() {
        let rig = rig().await;
        let (status, body) = send(
            &rig,
            authed(&rig, "GET", "/ipc/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["manifest"]["host"]["name"], "FAKE-PC");
        assert!(body["session"].is_null());
        assert_eq!(body["service"]["agent_connected"], false);

        let (status, body) = send(
            &rig,
            authed(&rig, "POST", "/ipc/pairing/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pairing"]["active"], true);
        assert_eq!(body["pairing"]["pin"].as_str().map(str::len), Some(6));

        // Case-insensitive scheme is fine.
        let r = req("GET", "/ipc/snapshot")
            .header("authorization", format!("bearer {}", rig.token))
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&rig, r).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn command_and_settings_return_coded_results() {
        let rig = rig().await;
        let cmd = serde_json::json!({"action":"set_switch","entity_id":"audio_mute","state":true});
        let r = authed(&rig, "POST", "/ipc/command")
            .header("content-type", "application/json")
            .body(Body::from(cmd.to_string()))
            .unwrap();
        let (status, body) = send(&rig, r).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], false);
        assert_eq!(body["code"], "no_agent");

        let bad = serde_json::json!({"scripts":[{"id":"BAD","name":"x","executable":"rel","args":[],"enabled":true}]});
        let r = authed(&rig, "PUT", "/ipc/settings")
            .header("content-type", "application/json")
            .body(Body::from(bad.to_string()))
            .unwrap();
        let (status, body) = send(&rig, r).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_argument");

        let ok = serde_json::json!({"temperature_unit":"fahrenheit"});
        let r = authed(&rig, "PUT", "/ipc/settings")
            .header("content-type", "application/json")
            .body(Body::from(ok.to_string()))
            .unwrap();
        let (status, body) = send(&rig, r).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["temperature_unit"], "fahrenheit");
    }

    #[tokio::test]
    async fn revoke_unknown_client_is_404() {
        let rig = rig().await;
        let (status, _) = send(
            &rig,
            authed(&rig, "DELETE", "/ipc/clients/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // --- Origin / Host / CORS --------------------------------------------------------------

    #[tokio::test]
    async fn any_origin_header_is_rejected_even_with_a_valid_token() {
        let rig = rig().await;
        for origin in [
            "http://evil.example",
            "null",
            "tauri://localhost",
            "http://127.0.0.1:9488",
        ] {
            let r = authed(&rig, "GET", "/ipc/snapshot")
                .header("origin", origin)
                .body(Body::empty())
                .unwrap();
            let (status, _) = send(&rig, r).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
        }
        // The WebSocket paths are protected the same way.
        let r = req("GET", "/ipc/ws")
            .header("origin", "http://evil.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&rig, r).await.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn bad_host_is_misdirected_even_with_a_valid_token() {
        let rig = rig().await;
        for host in [
            "evil.example",
            "evil.example:9488",
            "127.0.0.1",
            "127.0.0.1:1",
            "localhost",
            "192.168.1.5:9488",
            "127.0.0.1:9488.evil.com",
        ] {
            let r = Request::builder()
                .method("GET")
                .uri("/ipc/snapshot")
                .header("host", host)
                .header("authorization", format!("Bearer {}", rig.token))
                .body(Body::empty())
                .unwrap();
            let (status, _) = send(&rig, r).await;
            assert_eq!(status, StatusCode::MISDIRECTED_REQUEST, "{host}");
        }
        // Missing Host entirely.
        let r = Request::builder()
            .uri("/ipc/snapshot")
            .header("authorization", format!("Bearer {}", rig.token))
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&rig, r).await.0, StatusCode::MISDIRECTED_REQUEST);
        // Allowed spellings.
        for host in ["127.0.0.1:9488", "localhost:9488", "LocalHost:9488"] {
            let r = Request::builder()
                .uri("/ipc/snapshot")
                .header("host", host)
                .header("authorization", format!("Bearer {}", rig.token))
                .body(Body::empty())
                .unwrap();
            assert_eq!(send(&rig, r).await.0, StatusCode::OK, "{host}");
        }
    }

    #[tokio::test]
    async fn no_cors_headers_and_preflight_is_not_answered() {
        let rig = rig().await;
        let r = req("OPTIONS", "/ipc/snapshot")
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap();
        let resp = build_ipc_router(Arc::clone(&rig.state))
            .oneshot(r)
            .await
            .unwrap();
        assert!(
            !resp
                .headers()
                .keys()
                .any(|k| k.as_str().starts_with("access-control-"))
        );
        assert_ne!(resp.status(), StatusCode::OK);
        let ok = authed(&rig, "GET", "/ipc/snapshot")
            .body(Body::empty())
            .unwrap();
        let resp = build_ipc_router(Arc::clone(&rig.state))
            .oneshot(ok)
            .await
            .unwrap();
        assert!(
            !resp
                .headers()
                .keys()
                .any(|k| k.as_str().starts_with("access-control-"))
        );
    }

    #[test]
    fn host_matching_is_exact() {
        assert!(host_allowed("127.0.0.1:9488", 9488));
        assert!(host_allowed(" LOCALHOST:9488 ", 9488));
        assert!(!host_allowed("127.0.0.1:9489", 9488));
        assert!(!host_allowed("[::1]:9488", 9488));
        assert!(!host_allowed("", 9488));
    }

    // --- WebSockets (real loopback server) -------------------------------------------------

    /// A rig whose state knows the real loopback port (the Host check depends on it), served by
    /// the real IPC router with a short first-frame auth timeout.
    async fn serve_rig() -> (TestRig, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let rig = rig_opts(RigOpts {
            ipc_port: addr.port(),
            ..RigOpts::default()
        })
        .await;
        let router = build_ipc_router(Arc::clone(&rig.state));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (rig, addr)
    }

    type Client = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn ws_connect(addr: SocketAddr, path: &str) -> Client {
        let (c, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{path}"))
            .await
            .unwrap();
        c
    }

    async fn next_text(c: &mut Client) -> Option<String> {
        loop {
            match tokio::time::timeout(Duration::from_secs(3), c.next()).await {
                Ok(Some(Ok(WsMsg::Text(t)))) => return Some(t.to_string()),
                Ok(Some(Ok(WsMsg::Ping(_) | WsMsg::Pong(_)))) => {}
                _ => return None,
            }
        }
    }

    async fn is_closed(c: &mut Client) -> bool {
        loop {
            match tokio::time::timeout(Duration::from_secs(3), c.next()).await {
                Ok(Some(Ok(WsMsg::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return true,
                Ok(Some(Ok(_))) => {}
                Err(_) => return false,
            }
        }
    }

    fn auth_frame(token: &str) -> WsMsg {
        WsMsg::Text(
            serde_json::json!({"type":"auth","token":token})
                .to_string()
                .into(),
        )
    }

    #[tokio::test]
    async fn ws_requires_first_frame_auth_and_times_out() {
        let (rig, addr) = serve_rig().await;

        // No frame at all: closed after the (shortened) auth timeout.
        let mut c = ws_connect(addr, "/ipc/ws").await;
        assert!(is_closed(&mut c).await, "silent client must be closed");

        // Wrong token.
        let mut c = ws_connect(addr, "/ipc/ws").await;
        c.send(auth_frame(&"0".repeat(64))).await.unwrap();
        assert!(is_closed(&mut c).await);

        // Not an auth frame.
        let mut c = ws_connect(addr, "/ipc/agent").await;
        c.send(WsMsg::Text(r#"{"type":"hello"}"#.into()))
            .await
            .unwrap();
        assert!(is_closed(&mut c).await);

        // Correct token on the UI socket: auth_ok then a snapshot.
        let mut c = ws_connect(addr, "/ipc/ws").await;
        c.send(auth_frame(&rig.token)).await.unwrap();
        let first = next_text(&mut c).await.unwrap();
        assert!(first.contains("auth_ok"), "{first}");
        let snap: IpcEvent = serde_json::from_str(&next_text(&mut c).await.unwrap()).unwrap();
        assert!(matches!(snap, IpcEvent::Snapshot(_)));
    }

    #[tokio::test]
    async fn ws_upgrade_with_origin_or_bad_host_is_refused() {
        let (_rig, addr) = serve_rig().await;
        let mut r = format!("ws://{addr}/ipc/ws").into_client_request().unwrap();
        r.headers_mut()
            .insert("origin", "http://evil.example".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(r).await.is_err());
        let mut r = format!("ws://{addr}/ipc/ws").into_client_request().unwrap();
        r.headers_mut()
            .insert("host", "evil.example".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(r).await.is_err());
    }

    #[tokio::test]
    async fn rotating_the_token_closes_live_sockets_and_invalidates_the_old_token() {
        let (rig, addr) = serve_rig().await;
        let mut c = ws_connect(addr, "/ipc/ws").await;
        c.send(auth_frame(&rig.token)).await.unwrap();
        assert!(next_text(&mut c).await.is_some());

        let new = crate::ipc_token::rotate(&rig.state.data_dir).unwrap();
        assert!(rig.state.ipc_token.reload());
        assert!(
            is_closed(&mut c).await,
            "rotation must close live IPC sockets"
        );

        let mut c = ws_connect(addr, "/ipc/ws").await;
        c.send(auth_frame(&rig.token)).await.unwrap();
        assert!(is_closed(&mut c).await, "old token must stop working");
        let mut c = ws_connect(addr, "/ipc/ws").await;
        c.send(auth_frame(&new)).await.unwrap();
        assert!(next_text(&mut c).await.is_some());
    }

    #[tokio::test]
    async fn agent_session_end_to_end() {
        let (rig, addr) = serve_rig().await;
        let mut a = ws_connect(addr, "/ipc/agent").await;
        a.send(auth_frame(&rig.token)).await.unwrap();
        assert!(matches!(
            serde_json::from_str::<DaemonToAgent>(&next_text(&mut a).await.unwrap()).unwrap(),
            DaemonToAgent::AuthOk
        ));
        assert!(matches!(
            serde_json::from_str::<DaemonToAgent>(&next_text(&mut a).await.unwrap()).unwrap(),
            DaemonToAgent::Scripts { .. }
        ));
        a.send(WsMsg::Text(
            serde_json::to_string(&AgentMessage::Hello {
                agent_version: "0.6.0".into(),
                os: "windows".into(),
                session_id: 1,
                user: "leo".into(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
        a.send(WsMsg::Text(
            serde_json::to_string(&AgentMessage::SessionState(SessionState {
                gaming_mode: true,
                ..SessionState::default()
            }))
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

        // The state shows up in the daemon, and a request is forwarded and answered.
        let st = Arc::clone(&rig.state);
        let mut seen = false;
        for _ in 0..40 {
            if st.snapshot().await.session.is_some_and(|s| s.gaming_mode) {
                seen = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(seen, "agent session_state must reach the snapshot");
        assert!(st.snapshot().await.service.agent_connected);

        let exec = tokio::spawn({
            let st = Arc::clone(&st);
            async move { st.sessions.execute(SessionAction::Lock).await }
        });
        let req_frame = loop {
            let txt = next_text(&mut a).await.unwrap();
            if let DaemonToAgent::Request { id, action } = serde_json::from_str(&txt).unwrap() {
                assert_eq!(action, SessionAction::Lock);
                break id;
            }
        };
        a.send(WsMsg::Text(
            serde_json::to_string(&AgentMessage::response(
                req_frame,
                CommandResult::ok_with("locked"),
            ))
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
        let r = exec.await.unwrap();
        assert!(r.ok, "{r:?}");

        // Disconnect clears the agent and the state.
        drop(a);
        for _ in 0..40 {
            if !st.snapshot().await.service.agent_connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let snap = st.snapshot().await;
        assert!(!snap.service.agent_connected);
        assert!(snap.session.is_none());
    }
}
