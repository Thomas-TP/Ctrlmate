//! LAN side (`:9487`, TLS 1.3): `/api/v1/info`, `/api/v1/pair`, `/ws` for Home Assistant.
//!
//! Hardening: connection and handshake limits (total and per IP), header-read timeout,
//! per-IP PIN throttling, sanitised client names, revocation closes live sockets, one connection
//! id per socket, `telemetry` permission enforced on every state frame, error replies for
//! unparseable commands and a full resync after a lagged broadcast.

use crate::auth::{PairError, issue_client_token, sanitize_client_name, verify_client_token};
use crate::state::{AppState, lock};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ctrlmate_protocol::{
    CommandResult, ControlCommand, DiscoveryInfo, PROTOCOL_VERSION, PairRequest, PairResponse,
    ResultCode, WireMessage,
};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing::{debug, info, warn};

pub const MAX_TOTAL_CONNECTIONS: usize = 64;
pub const MAX_CONNECTIONS_PER_IP: usize = 8;
pub const MAX_PAIRED_CLIENTS: usize = 16;
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const WS_AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const WS_PING_EVERY: Duration = Duration::from_secs(30);
const WS_DEAD_AFTER: Duration = Duration::from_secs(90);
const MAX_WS_MESSAGE: usize = 64 * 1024;
const MAX_INFLIGHT_COMMANDS: usize = 4;

// ---------------------------------------------------------------------------------------------
// Connection limits
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct LimiterInner {
    total: usize,
    per_ip: HashMap<IpAddr, usize>,
}

/// Limits concurrent TCP connections (which includes connections still in the TLS handshake).
#[derive(Clone)]
pub struct ConnLimiter {
    inner: Arc<Mutex<LimiterInner>>,
    max_total: usize,
    max_per_ip: usize,
}

pub struct ConnPermit {
    inner: Arc<Mutex<LimiterInner>>,
    ip: IpAddr,
}

impl ConnLimiter {
    pub fn new(max_total: usize, max_per_ip: usize) -> Self {
        Self {
            inner: Arc::default(),
            max_total,
            max_per_ip,
        }
    }

    pub fn try_acquire(&self, ip: IpAddr) -> Option<ConnPermit> {
        let mut g = lock(&self.inner);
        let per = g.per_ip.get(&ip).copied().unwrap_or(0);
        if g.total >= self.max_total || per >= self.max_per_ip {
            return None;
        }
        g.total += 1;
        *g.per_ip.entry(ip).or_insert(0) += 1;
        Some(ConnPermit {
            inner: Arc::clone(&self.inner),
            ip,
        })
    }
}

impl Drop for ConnPermit {
    fn drop(&mut self) {
        let mut g = lock(&self.inner);
        g.total = g.total.saturating_sub(1);
        if let Some(n) = g.per_ip.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.per_ip.remove(&self.ip);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------------------------

pub fn build_ha_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/info", get(ha_info))
        .route("/api/v1/pair", post(ha_pair))
        .route("/ws", get(ha_ws_upgrade))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state)
}

/// Serves one already-established (TLS) stream with HTTP/1.1 + WebSocket upgrades.
pub async fn serve_stream<IO>(io: IO, router: Router, peer: SocketAddr)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |mut req: hyper::Request<Incoming>| {
        req.extensions_mut().insert(ConnectInfo(peer));
        router.clone().oneshot(req.map(axum::body::Body::new))
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades();
    if let Err(err) = conn.await {
        let msg = err.to_string();
        if !msg.contains("connection closed") && !msg.contains("timeout") {
            debug!("LAN connection error from {peer}: {msg}");
        }
    }
}

/// Accept loop of the LAN listener. Returns when `shutdown` turns `true`.
pub async fn serve_lan(
    state: Arc<AppState>,
    listener: TcpListener,
    acceptor: TlsAcceptor,
    mut shutdown: watch::Receiver<bool>,
) {
    let router = build_ha_router(state);
    let limiter = ConnLimiter::new(MAX_TOTAL_CONNECTIONS, MAX_CONNECTIONS_PER_IP);
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
            accepted = listener.accept() => {
                let (tcp, peer) = match accepted {
                    Ok(v) => v,
                    Err(err) => {
                        warn!("TCP accept error: {err}");
                        // Avoid a hot loop on persistent errors (e.g. out of descriptors).
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Some(permit) = limiter.try_acquire(peer.ip()) else {
                    debug!("Connection limit reached; dropping {peer}");
                    drop(tcp);
                    continue;
                };
                let acceptor = acceptor.clone();
                let router = router.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let tls = match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(err)) => {
                            debug!("TLS handshake failed from {peer}: {err}");
                            return;
                        }
                        Err(_) => {
                            debug!("TLS handshake timed out from {peer}");
                            return;
                        }
                    };
                    serve_stream(tls, router, peer).await;
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------------------------

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

/// Minimal identity for strangers; hostname and MAC only for a valid paired bearer token.
async fn ha_info(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Json<DiscoveryInfo> {
    let paired = match bearer(&headers) {
        Some(t) => verify_client_token(&state.config.read().await.paired_clients, t).is_some(),
        None => false,
    };
    let device_id = state.config.read().await.device_id.clone();
    let manifest = state.manifest.read().await;
    let pairing_active = lock(&state.pairing).is_active();
    Json(DiscoveryInfo {
        protocol_version: PROTOCOL_VERSION,
        device_id,
        sw_version: manifest.host.sw_version.clone(),
        cert_fingerprint: state.cert_fingerprint.clone(),
        pairing_active,
        hostname: paired.then(|| manifest.host.name.clone()),
        mac_address: if paired {
            manifest.host.mac_address.clone()
        } else {
            None
        },
    })
}

fn pair_error_response(err: &PairError) -> Response {
    let body = serde_json::json!({
        "ok": false,
        "code": err.code(),
        "error": err.code(),
        "retry_after_s": err.retry_after_secs(),
    });
    match err {
        PairError::Throttled { retry_after_secs } => (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after_secs.to_string())],
            Json(body),
        )
            .into_response(),
        PairError::NoSession | PairError::InvalidPin { .. } => {
            (StatusCode::UNAUTHORIZED, Json(body)).into_response()
        }
    }
}

async fn ha_pair(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(req): Json<PairRequest>,
) -> Response {
    // The session is only consumed once the PIN is right, so a stranger's wrong guesses never
    // end the real user's session (see `auth` for the per-IP budget).
    let verdict = lock(&state.pairing).verify_and_consume(peer.ip(), &req.pin);
    if let Err(err) = verdict {
        warn!("Pairing attempt from {peer} refused: {}", err.code());
        return pair_error_response(&err);
    }

    let (token, stored) = {
        let mut cfg = state.config.write().await;
        if cfg.paired_clients.len() >= MAX_PAIRED_CLIENTS {
            return (
                StatusCode::CONFLICT,
                Json(CommandResult::err_with(
                    ResultCode::Busy,
                    "too many paired clients",
                )),
            )
                .into_response();
        }
        let (tok, st) = issue_client_token(&req.client_name);
        cfg.paired_clients.push(st.clone());
        if let Err(err) = cfg.save(&state.data_dir) {
            warn!("Failed persisting paired client: {err:#}");
        }
        (tok, st)
    };

    info!(
        "Paired new Home Assistant client '{}' ({}) from {peer}",
        stored.client_name, stored.client_id
    );
    let device_id = state.config.read().await.device_id.clone();
    let manifest = state.manifest.read().await;
    let resp = PairResponse {
        client_id: stored.client_id,
        token,
        device_id,
        hostname: manifest.host.name.clone(),
        mac_address: manifest.host.mac_address.clone(),
        cert_fingerprint: state.cert_fingerprint.clone(),
    };
    drop(manifest);
    state.notify_ui().await;
    (StatusCode::OK, Json(resp)).into_response()
}

// ---------------------------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------------------------

async fn ha_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| handle_ha_ws(socket, state, peer.ip()))
}

type WsSink = SplitSink<WebSocket, Message>;

async fn send_wire(sink: &mut WsSink, msg: &WireMessage) -> Result<(), ()> {
    let txt = serde_json::to_string(msg).map_err(|_| ())?;
    sink.send(Message::Text(txt.into())).await.map_err(|_| ())
}

fn request_id_of(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("request_id")?.as_str().map(str::to_string))
        .unwrap_or_default()
        .chars()
        .take(128)
        .collect()
}

fn result_frame(request_id: String, r: CommandResult) -> WireMessage {
    WireMessage::CommandResult {
        request_id,
        ok: r.ok,
        code: r.code,
        message: r.message,
    }
}

/// Removes the connection from the registry even if the task is cancelled.
struct ConnGuard {
    state: Arc<AppState>,
    id: u64,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.state.conns.unregister(self.id);
    }
}

async fn handle_ha_ws(socket: WebSocket, state: Arc<AppState>, remote: IpAddr) {
    let (mut sink, mut stream) = socket.split();

    // The hostname is not sent before authentication.
    let hello = WireMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        device_id: state.config.read().await.device_id.clone(),
        hostname: String::new(),
        sw_version: env!("CARGO_PKG_VERSION").to_string(),
        cert_fingerprint: state.cert_fingerprint.clone(),
    };
    if send_wire(&mut sink, &hello).await.is_err() {
        return;
    }

    let first = tokio::time::timeout(WS_AUTH_TIMEOUT, stream.next()).await;
    let (client_id, client_name) = match first {
        Ok(Some(Ok(Message::Text(txt)))) => match serde_json::from_str::<WireMessage>(&txt) {
            Ok(WireMessage::Auth { token, client_name }) => {
                let cfg = state.config.read().await;
                match verify_client_token(&cfg.paired_clients, &token) {
                    Some(cid) => {
                        let stored = cfg
                            .paired_clients
                            .iter()
                            .find(|c| c.client_id == cid)
                            .map_or_else(
                                || sanitize_client_name(&client_name),
                                |c| c.client_name.clone(),
                            );
                        (cid, stored)
                    }
                    None => {
                        let _ = send_wire(
                            &mut sink,
                            &WireMessage::AuthError {
                                reason: "invalid_token".into(),
                            },
                        )
                        .await;
                        return;
                    }
                }
            }
            _ => {
                let _ = send_wire(
                    &mut sink,
                    &WireMessage::AuthError {
                        reason: "auth_expected".into(),
                    },
                )
                .await;
                return;
            }
        },
        _ => return,
    };

    let (conn_id, mut kill) = state.conns.register(client_id.clone());
    let _guard = ConnGuard {
        state: Arc::clone(&state),
        id: conn_id,
    };
    info!(
        "Home Assistant client '{client_name}' ({client_id}) connected from {remote} [conn {conn_id}]"
    );

    if send_wire(
        &mut sink,
        &WireMessage::AuthOk {
            client_id: client_id.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }

    // Subscribe before taking the snapshot so nothing is lost in between.
    let mut rx = state.ha_tx.subscribe();
    let (manifest, all_states) = state.resync_frames().await;
    if send_wire(&mut sink, &manifest).await.is_err() {
        return;
    }
    if state.telemetry_allowed() && send_wire(&mut sink, &all_states).await.is_err() {
        return;
    }
    // A new listener wants fresh data now and the delta filter must not hide it from others.
    lock(&state.poller).reset();
    state.sampler_wake.notify_one();
    state.notify_ui().await;

    let (out_tx, mut out_rx) = mpsc::channel::<WireMessage>(32);
    let inflight = Arc::new(Semaphore::new(MAX_INFLIGHT_COMMANDS));
    let mut shutdown = state.shutdown.subscribe();
    let mut ping = tokio::time::interval(WS_PING_EVERY);
    ping.tick().await;
    let mut last_rx = Instant::now();

    loop {
        tokio::select! {
            _ = kill.changed() => {
                info!("Closing connection {conn_id} of client {client_id}: token revoked");
                let _ = send_wire(&mut sink, &WireMessage::AuthError { reason: "token_revoked".into() }).await;
                let _ = sink.send(Message::Close(Some(CloseFrame { code: 4401, reason: "revoked".into() }))).await;
                break;
            }
            _ = shutdown.changed() => {
                let _ = sink.send(Message::Close(Some(CloseFrame { code: 1001, reason: "going away".into() }))).await;
                break;
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > WS_DEAD_AFTER { break; }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
            Some(frame) = out_rx.recv() => {
                if send_wire(&mut sink, &frame).await.is_err() { break; }
            }
            broadcast_msg = rx.recv() => {
                match broadcast_msg {
                    Ok(WireMessage::StateBatch { .. }) if !state.telemetry_allowed() => {}
                    Ok(msg) => {
                        if send_wire(&mut sink, &msg).await.is_err() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Missed deltas: send the full picture instead of guessing.
                        debug!("Connection {conn_id} lagged by {n} frames; resyncing");
                        let (m, s) = state.resync_frames().await;
                        if send_wire(&mut sink, &m).await.is_err() { break; }
                        if state.telemetry_allowed() && send_wire(&mut sink, &s).await.is_err() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        last_rx = Instant::now();
                        match serde_json::from_str::<WireMessage>(&txt) {
                            Ok(WireMessage::Command { request_id, command }) => {
                                spawn_command(&state, &inflight, &out_tx, request_id, command);
                            }
                            Ok(WireMessage::Ping { ts }) => {
                                if send_wire(&mut sink, &WireMessage::Pong { ts }).await.is_err() { break; }
                            }
                            Ok(_) => {}
                            Err(_) => {
                                let reply = result_frame(
                                    request_id_of(&txt),
                                    CommandResult::err_with(ResultCode::InvalidArgument, "unparseable frame"),
                                );
                                if send_wire(&mut sink, &reply).await.is_err() { break; }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => { last_rx = Instant::now(); }
                }
            }
        }
    }

    info!("Home Assistant client ({client_id}) connection {conn_id} closed");
    drop(_guard);
    state.notify_ui().await;
}

fn spawn_command(
    state: &Arc<AppState>,
    inflight: &Arc<Semaphore>,
    out_tx: &mpsc::Sender<WireMessage>,
    request_id: String,
    command: ControlCommand,
) {
    let Ok(permit) = Arc::clone(inflight).try_acquire_owned() else {
        let _ = out_tx.try_send(result_frame(
            request_id,
            CommandResult::err(ResultCode::Busy),
        ));
        return;
    };
    let state = Arc::clone(state);
    let out_tx = out_tx.clone();
    tokio::spawn(async move {
        let result = state.execute_command(command, true).await;
        let _ = out_tx.send(result_frame(request_id, result)).await;
        drop(permit);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testutil::{TestRig, rig};
    use axum::body::Body;
    use axum::http::Request;
    use ctrlmate_protocol::{EntityState, EntityValue, RemotePermissions, SettingsUpdate};
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    fn peer(n: u8) -> SocketAddr {
        SocketAddr::from(([192, 168, 1, n], 40000))
    }

    async fn call(
        state: &Arc<AppState>,
        req: Request<Body>,
        from: SocketAddr,
    ) -> (StatusCode, HeaderMap, serde_json::Value) {
        let mut req = req;
        req.extensions_mut().insert(ConnectInfo(from));
        let resp = build_ha_router(Arc::clone(state))
            .oneshot(req)
            .await
            .unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (parts.status, parts.headers, v)
    }

    fn pair_req(pin: &str, name: &str) -> Request<Body> {
        Request::post("/api/v1/pair")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"pin": pin, "client_name": name}).to_string(),
            ))
            .unwrap()
    }

    fn start_pairing(state: &Arc<AppState>) -> String {
        lock(&state.pairing).start().pin
    }

    fn other_pin(pin: &str) -> &'static str {
        if pin == "000000" { "111111" } else { "000000" }
    }

    // --- limiter ----------------------------------------------------------------------------

    #[test]
    fn limiter_caps_per_ip_and_total_and_releases_on_drop() {
        let l = ConnLimiter::new(3, 2);
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let p1 = l.try_acquire(a).unwrap();
        let _p2 = l.try_acquire(a).unwrap();
        assert!(l.try_acquire(a).is_none(), "per-IP cap");
        let _p3 = l.try_acquire(b).unwrap();
        assert!(l.try_acquire(b).is_none(), "total cap");
        drop(p1);
        assert!(l.try_acquire(b).is_some(), "slot released");
    }

    // --- /api/v1/info -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_is_minimal_for_strangers_and_full_for_paired_clients() {
        let rig = rig().await;
        let st = &rig.state;
        let (status, _, v) = call(
            st,
            Request::get("/api/v1/info").body(Body::empty()).unwrap(),
            peer(9),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(v.get("hostname").is_none(), "{v}");
        assert!(v.get("mac_address").is_none(), "{v}");
        assert_eq!(v["cert_fingerprint"], "ab".repeat(32));

        // A bogus bearer is still a stranger.
        let (_, _, v) = call(
            st,
            Request::get("/api/v1/info")
                .header("authorization", "Bearer nope")
                .body(Body::empty())
                .unwrap(),
            peer(9),
        )
        .await;
        assert!(v.get("hostname").is_none());

        let (token, stored) = issue_client_token("HA");
        st.config.write().await.paired_clients.push(stored);
        let (_, _, v) = call(
            st,
            Request::get("/api/v1/info")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
            peer(9),
        )
        .await;
        assert_eq!(v["hostname"], "FAKE-PC");
        assert!(v["mac_address"].is_string());
    }

    // --- pairing ----------------------------------------------------------------------------

    #[tokio::test]
    async fn pairing_succeeds_with_the_pin_and_stores_only_a_hash() {
        let rig = rig().await;
        let st = &rig.state;
        let pin = start_pairing(st);
        let (status, _, v) = call(st, pair_req(&pin, "  Home\u{0}Assistant \n"), peer(20)).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        let token = v["token"].as_str().unwrap().to_string();
        assert!(token.starts_with("cm2_"));
        let cfg = st.config.read().await;
        assert_eq!(cfg.paired_clients.len(), 1);
        assert_eq!(cfg.paired_clients[0].client_name, "Home Assistant");
        let disk = std::fs::read_to_string(st.data_dir.file("config.json")).unwrap();
        assert!(
            !disk.contains(&token),
            "plaintext token must never be stored"
        );
        // Session is consumed.
        drop(cfg);
        let (status, _, _) = call(st, pair_req(&pin, "x"), peer(20)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_stranger_cannot_burn_the_real_users_pairing_session() {
        let rig = rig().await;
        let st = &rig.state;
        let pin = start_pairing(st);
        let bad = other_pin(&pin);
        // The stranger hammers the endpoint.
        let mut throttled = false;
        for _ in 0..40 {
            let (status, headers, v) = call(st, pair_req(bad, "evil"), peer(66)).await;
            assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::TOO_MANY_REQUESTS);
            if status == StatusCode::TOO_MANY_REQUESTS {
                throttled = true;
                assert!(headers.contains_key(header::RETRY_AFTER));
                assert_eq!(v["code"], "throttled");
            }
        }
        assert!(throttled);
        // The PIN is still valid for the real user on another address.
        assert!(lock(&st.pairing).is_active());
        let (status, _, v) = call(st, pair_req(&pin, "HA"), peer(20)).await;
        assert_eq!(status, StatusCode::OK, "{v}");
    }

    #[tokio::test]
    async fn pairing_without_a_session_is_refused() {
        let rig = rig().await;
        let (status, _, v) = call(&rig.state, pair_req("123456", "x"), peer(1)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(v["code"], "no_session");
    }

    #[tokio::test]
    async fn oversized_pair_bodies_are_rejected() {
        let rig = rig().await;
        let big = serde_json::json!({"pin": "1", "client_name": "x".repeat(100_000)}).to_string();
        let req = Request::post("/api/v1/pair")
            .header("content-type", "application/json")
            .body(Body::from(big))
            .unwrap();
        let (status, _, _) = call(&rig.state, req, peer(1)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn paired_client_count_is_capped() {
        let rig = rig().await;
        let st = &rig.state;
        for _ in 0..MAX_PAIRED_CLIENTS {
            st.config
                .write()
                .await
                .paired_clients
                .push(issue_client_token("x").1);
        }
        let pin = start_pairing(st);
        let (status, _, _) = call(st, pair_req(&pin, "one too many"), peer(2)).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    // --- WebSocket --------------------------------------------------------------------------

    struct Server {
        addr: SocketAddr,
    }

    async fn start_server(state: &Arc<AppState>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = build_ha_router(Arc::clone(state));
        tokio::spawn(async move {
            loop {
                let Ok((tcp, peer)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(serve_stream(tcp, router.clone(), peer));
            }
        });
        Server { addr }
    }

    type Client = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn connect(server: &Server) -> Client {
        let (c, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", server.addr))
            .await
            .unwrap();
        c
    }

    async fn recv_wire(c: &mut Client) -> Option<WireMessage> {
        loop {
            match tokio::time::timeout(Duration::from_secs(3), c.next()).await {
                Ok(Some(Ok(WsMsg::Text(t)))) => return serde_json::from_str(&t).ok(),
                Ok(Some(Ok(WsMsg::Ping(_) | WsMsg::Pong(_)))) => {}
                _ => return None,
            }
        }
    }

    async fn send_wire_c(c: &mut Client, m: &WireMessage) {
        c.send(WsMsg::Text(serde_json::to_string(m).unwrap().into()))
            .await
            .unwrap();
    }

    async fn paired(rig: &TestRig) -> (String, String) {
        let (token, stored) = issue_client_token("HA");
        let id = stored.client_id.clone();
        rig.state.config.write().await.paired_clients.push(stored);
        (token, id)
    }

    async fn auth(c: &mut Client, token: &str) {
        assert!(matches!(
            recv_wire(c).await,
            Some(WireMessage::Hello { .. })
        ));
        send_wire_c(
            c,
            &WireMessage::Auth {
                token: token.into(),
                client_name: "ha".into(),
            },
        )
        .await;
        assert!(matches!(
            recv_wire(c).await,
            Some(WireMessage::AuthOk { .. })
        ));
        assert!(matches!(recv_wire(c).await, Some(WireMessage::Manifest(_))));
        assert!(matches!(
            recv_wire(c).await,
            Some(WireMessage::StateBatch { .. })
        ));
    }

    #[tokio::test]
    async fn hello_leaks_no_hostname_and_bad_tokens_get_auth_error() {
        let rig = rig().await;
        let server = start_server(&rig.state).await;
        let mut c = connect(&server).await;
        let Some(WireMessage::Hello {
            hostname,
            device_id,
            ..
        }) = recv_wire(&mut c).await
        else {
            panic!("hello expected");
        };
        assert!(hostname.is_empty());
        assert_eq!(device_id, rig.state.config.read().await.device_id);
        send_wire_c(
            &mut c,
            &WireMessage::Auth {
                token: "cm2_wrong".into(),
                client_name: "x".into(),
            },
        )
        .await;
        assert!(matches!(
            recv_wire(&mut c).await,
            Some(WireMessage::AuthError { .. })
        ));
    }

    #[tokio::test]
    async fn commands_get_results_with_codes_and_garbage_gets_an_error_reply() {
        let rig = rig().await;
        let (token, _) = paired(&rig).await;
        let server = start_server(&rig.state).await;
        let mut c = connect(&server).await;
        auth(&mut c, &token).await;

        send_wire_c(
            &mut c,
            &WireMessage::Command {
                request_id: "r1".into(),
                command: ControlCommand::SetSwitch {
                    entity_id: "audio_mute".into(),
                    state: true,
                },
            },
        )
        .await;
        let Some(WireMessage::CommandResult {
            request_id,
            ok,
            code,
            ..
        }) = recv_wire(&mut c).await
        else {
            panic!("result expected");
        };
        assert_eq!(
            (request_id.as_str(), ok, code.as_str()),
            ("r1", false, "no_agent")
        );

        // Unparseable frame carrying a request id: error reply with the id echoed.
        c.send(WsMsg::Text(
            r#"{"type":"command","request_id":"r2","command":{"action":"nope"}}"#.into(),
        ))
        .await
        .unwrap();
        let Some(WireMessage::CommandResult {
            request_id,
            ok,
            code,
            ..
        }) = recv_wire(&mut c).await
        else {
            panic!("error reply expected");
        };
        assert_eq!(
            (request_id.as_str(), ok, code.as_str()),
            ("r2", false, "invalid_argument")
        );

        // Pure garbage too.
        c.send(WsMsg::Text("not json".into())).await.unwrap();
        let Some(WireMessage::CommandResult { ok, code, .. }) = recv_wire(&mut c).await else {
            panic!("error reply expected");
        };
        assert!(!ok);
        assert_eq!(code, "invalid_argument");

        // The connection survives and still answers pings.
        send_wire_c(&mut c, &WireMessage::Ping { ts: 7 }).await;
        assert!(matches!(
            recv_wire(&mut c).await,
            Some(WireMessage::Pong { ts: 7 })
        ));
    }

    #[tokio::test]
    async fn revoking_a_client_closes_its_live_sockets() {
        let rig = rig().await;
        let (token, id) = paired(&rig).await;
        let server = start_server(&rig.state).await;
        let mut c1 = connect(&server).await;
        auth(&mut c1, &token).await;
        let mut c2 = connect(&server).await;
        auth(&mut c2, &token).await;

        // Connections are keyed by id: one client, two connections.
        let snap = rig.state.snapshot().await;
        assert_eq!(snap.paired_clients[0].connections, 2);
        assert!(snap.paired_clients[0].connected);

        assert!(rig.state.revoke_client(&id).await);
        for c in [&mut c1, &mut c2] {
            let mut closed = false;
            for _ in 0..10 {
                match tokio::time::timeout(Duration::from_secs(3), c.next()).await {
                    Ok(Some(Ok(WsMsg::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => {
                        closed = true;
                        break;
                    }
                    Ok(Some(Ok(_))) => {}
                    Err(_) => break,
                }
            }
            assert!(closed, "socket must be closed after revocation");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(rig.state.conns.len(), 0);
        // The old token no longer authenticates.
        let mut c3 = connect(&server).await;
        assert!(matches!(
            recv_wire(&mut c3).await,
            Some(WireMessage::Hello { .. })
        ));
        send_wire_c(
            &mut c3,
            &WireMessage::Auth {
                token,
                client_name: "ha".into(),
            },
        )
        .await;
        assert!(matches!(
            recv_wire(&mut c3).await,
            Some(WireMessage::AuthError { .. })
        ));
    }

    fn batch(id: &str, v: f64) -> WireMessage {
        WireMessage::StateBatch {
            states: vec![EntityState {
                id: id.into(),
                value: EntityValue::Float(v),
                attributes: Default::default(),
                updated_at: chrono::Utc::now(),
            }],
        }
    }

    #[tokio::test]
    async fn telemetry_permission_mutes_state_frames_and_resyncs_when_restored() {
        let rig = rig().await;
        let st = &rig.state;
        let (token, _) = paired(&rig).await;
        let server = start_server(st).await;
        let mut c = connect(&server).await;
        auth(&mut c, &token).await;

        // Delivered while telemetry is on.
        let _ = st.ha_tx.send(batch("cpu_0_load", 1.0));
        assert!(matches!(
            recv_wire(&mut c).await,
            Some(WireMessage::StateBatch { .. })
        ));

        let mut perms = RemotePermissions {
            telemetry: false,
            ..RemotePermissions::default()
        };
        st.apply_settings(SettingsUpdate {
            permissions: Some(perms.clone()),
            ..SettingsUpdate::default()
        })
        .await;
        // Drain anything produced by the settings change itself (manifest etc.).
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(150), c.next()).await {}

        let _ = st.ha_tx.send(batch("cpu_0_load", 2.0));
        assert!(
            tokio::time::timeout(Duration::from_millis(300), c.next())
                .await
                .is_err(),
            "no state frame may reach a client without the telemetry permission"
        );
        // Commands and pings still work.
        send_wire_c(&mut c, &WireMessage::Ping { ts: 1 }).await;
        assert!(matches!(
            recv_wire(&mut c).await,
            Some(WireMessage::Pong { .. })
        ));

        perms.telemetry = true;
        st.apply_settings(SettingsUpdate {
            permissions: Some(perms),
            ..SettingsUpdate::default()
        })
        .await;
        let mut full = false;
        for _ in 0..6 {
            if let Some(WireMessage::StateBatch { states }) = recv_wire(&mut c).await
                && states.len() > 3
            {
                full = true;
                break;
            }
        }
        assert!(full, "restoring telemetry must resend the full state");
    }

    #[tokio::test]
    async fn lagged_receivers_get_a_full_resync() {
        let rig = rig().await;
        let st = &rig.state;
        let (token, _) = paired(&rig).await;
        let server = start_server(st).await;
        let mut c = connect(&server).await;
        auth(&mut c, &token).await;
        // Overflow the broadcast buffer without yielding so the connection task falls behind.
        for i in 0..2000 {
            let _ = st.ha_tx.send(batch("cpu_0_load", f64::from(i)));
        }
        let mut saw_manifest_resync = false;
        for _ in 0..2100 {
            match recv_wire(&mut c).await {
                Some(WireMessage::Manifest(_)) => {
                    saw_manifest_resync = true;
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        assert!(
            saw_manifest_resync,
            "lag must trigger manifest + full state resync"
        );
    }

    #[tokio::test]
    async fn slow_command_does_not_block_the_connection() {
        let rig = rig().await;
        let (token, _) = paired(&rig).await;
        // Agent attached but silent: the command takes the 300 ms agent timeout.
        let _att = rig.state.sessions.agent.attach();
        let server = start_server(&rig.state).await;
        let mut c = connect(&server).await;
        auth(&mut c, &token).await;
        send_wire_c(
            &mut c,
            &WireMessage::Command {
                request_id: "slow".into(),
                command: ControlCommand::SetSwitch {
                    entity_id: "audio_mute".into(),
                    state: true,
                },
            },
        )
        .await;
        send_wire_c(&mut c, &WireMessage::Ping { ts: 5 }).await;
        // The pong arrives before the slow command's result.
        assert!(matches!(
            recv_wire(&mut c).await,
            Some(WireMessage::Pong { ts: 5 })
        ));
        let Some(WireMessage::CommandResult { code, .. }) = recv_wire(&mut c).await else {
            panic!("result expected");
        };
        assert_eq!(code, "agent_timeout");
    }

    #[test]
    fn request_id_extraction_is_bounded() {
        assert_eq!(request_id_of(r#"{"request_id":"abc"}"#), "abc");
        assert_eq!(request_id_of("garbage"), "");
        assert_eq!(
            request_id_of(&format!(r#"{{"request_id":"{}"}}"#, "x".repeat(1000))).len(),
            128
        );
    }
}
