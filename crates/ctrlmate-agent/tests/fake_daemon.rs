//! Integration tests: the agent client against a fake in-process daemon WebSocket.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ctrlmate_agent::client::{ClientConfig, SessionApi, run};
use ctrlmate_protocol::{
    AgentMessage, AllowlistedScript, CommandResult, DaemonToAgent, ResultCode, SessionAction,
    SessionState,
};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

#[derive(Default)]
struct MockApi {
    gaming: Mutex<bool>,
    executed: Mutex<Vec<SessionAction>>,
    scripts: Mutex<Vec<AllowlistedScript>>,
    samples: AtomicU32,
}

impl SessionApi for MockApi {
    fn sample(&self) -> SessionState {
        self.samples.fetch_add(1, Ordering::SeqCst);
        SessionState {
            gaming_mode: *self.gaming.lock().unwrap(),
            ..SessionState::default()
        }
    }
    fn execute(&self, action: &SessionAction) -> CommandResult {
        self.executed.lock().unwrap().push(action.clone());
        match action {
            SessionAction::RunScript { script_id } => {
                if self
                    .scripts
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|s| &s.id == script_id)
                {
                    CommandResult::ok_with("ran")
                } else {
                    CommandResult::err(ResultCode::InvalidArgument)
                }
            }
            _ => CommandResult::ok(),
        }
    }
    fn set_scripts(&self, scripts: Vec<AllowlistedScript>) {
        *self.scripts.lock().unwrap() = scripts;
    }
}

fn cfg(addr: SocketAddr, token: Arc<Mutex<String>>) -> ClientConfig {
    let mut c = ClientConfig::new(
        addr.to_string(),
        Arc::new(move || Some(token.lock().unwrap().clone())),
    );
    c.sample_interval = Duration::from_millis(100);
    c.heartbeat = Duration::from_millis(400);
    c.backoff_min = Duration::from_millis(50);
    c.backoff_max = Duration::from_millis(200);
    c.replaced_backoff = Duration::from_millis(300);
    c.auth_timeout = Duration::from_millis(800);
    c
}

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn bind() -> (TcpListener, SocketAddr) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    (l, a)
}

async fn accept(l: &TcpListener) -> Ws {
    let (s, _) = l.accept().await.unwrap();
    tokio_tungstenite::accept_async(s).await.unwrap()
}

async fn next_text(ws: &mut Ws, ms: u64) -> Option<String> {
    loop {
        match tokio::time::timeout(Duration::from_millis(ms), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return Some(t.to_string()),
            Ok(Some(Ok(_))) => {}
            _ => return None,
        }
    }
}

async fn send<T: serde::Serialize>(ws: &mut Ws, v: &T) {
    ws.send(Message::Text(serde_json::to_string(v).unwrap().into()))
        .await
        .unwrap();
}

/// Accepts, checks the first frame is the auth frame with `token`, and answers `auth_ok`.
async fn handshake(ws: &mut Ws, token: &str) {
    let first = next_text(ws, 3000).await.expect("auth frame");
    let v: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(
        v["type"], "auth",
        "the first frame must be the auth frame: {first}"
    );
    assert_eq!(v["token"], token);
    send(ws, &DaemonToAgent::AuthOk).await;
}

fn start(
    cfg: ClientConfig,
    api: Arc<MockApi>,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = watch::channel(false);
    let h = tokio::spawn(async move { run(cfg, api, rx).await });
    (tx, h)
}

#[tokio::test]
async fn hello_state_requests_and_script_allowlist() {
    let (l, addr) = bind().await;
    let api = Arc::new(MockApi::default());
    let (stop, h) = start(
        cfg(addr, Arc::new(Mutex::new("tok".into()))),
        Arc::clone(&api),
    );

    let mut ws = accept(&l).await;
    handshake(&mut ws, "tok").await;
    send(
        &mut ws,
        &DaemonToAgent::Scripts {
            scripts: vec![AllowlistedScript {
                id: "backup".into(),
                name: "Backup".into(),
                executable: "/x".into(),
                args: vec![],
                working_dir: None,
                enabled: true,
            }],
        },
    )
    .await;

    // hello first, then a session_state.
    let hello: AgentMessage =
        serde_json::from_str(&next_text(&mut ws, 2000).await.unwrap()).unwrap();
    assert!(matches!(hello, AgentMessage::Hello { ref os, .. } if !os.is_empty()));
    let st: AgentMessage = serde_json::from_str(&next_text(&mut ws, 2000).await.unwrap()).unwrap();
    assert!(matches!(st, AgentMessage::SessionState(_)));

    // Requests are executed and answered with the same id.
    send(
        &mut ws,
        &DaemonToAgent::Request {
            id: "r1".into(),
            action: SessionAction::Notify {
                title: "t".into(),
                body: "b".into(),
            },
        },
    )
    .await;
    send(
        &mut ws,
        &DaemonToAgent::Request {
            id: "r2".into(),
            action: SessionAction::RunScript {
                script_id: "backup".into(),
            },
        },
    )
    .await;
    send(
        &mut ws,
        &DaemonToAgent::Request {
            id: "r3".into(),
            action: SessionAction::RunScript {
                script_id: "../../evil".into(),
            },
        },
    )
    .await;
    let mut responses = std::collections::BTreeMap::new();
    while responses.len() < 3 {
        let t = next_text(&mut ws, 3000).await.expect("response");
        if let AgentMessage::Response { id, ok, code, .. } = serde_json::from_str(&t).unwrap() {
            responses.insert(id, (ok, code));
        }
    }
    assert_eq!(responses["r1"], (true, "ok".to_string()));
    assert_eq!(responses["r2"], (true, "ok".to_string()));
    assert_eq!(responses["r3"], (false, "invalid_argument".to_string()));
    assert_eq!(api.executed.lock().unwrap().len(), 3);

    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), h)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn state_is_sent_on_change_and_at_least_every_heartbeat() {
    let (l, addr) = bind().await;
    let api = Arc::new(MockApi::default());
    let (stop, h) = start(
        cfg(addr, Arc::new(Mutex::new("tok".into()))),
        Arc::clone(&api),
    );
    let mut ws = accept(&l).await;
    handshake(&mut ws, "tok").await;

    let mut count_states = 0;
    let started = std::time::Instant::now();
    // Unchanged state: only heartbeat frames (400 ms), far fewer than one per 100 ms sample.
    while started.elapsed() < Duration::from_millis(1500) {
        if let Some(t) = next_text(&mut ws, 200).await
            && let Ok(AgentMessage::SessionState(_)) = serde_json::from_str(&t)
        {
            count_states += 1;
        }
    }
    assert!(
        (2..=6).contains(&count_states),
        "heartbeat cadence, got {count_states}"
    );

    // A change goes out well before the next heartbeat would.
    *api.gaming.lock().unwrap() = true;
    let t0 = std::time::Instant::now();
    loop {
        let t = next_text(&mut ws, 2000).await.expect("state after change");
        if let AgentMessage::SessionState(s) = serde_json::from_str(&t).unwrap()
            && s.gaming_mode
        {
            break;
        }
    }
    assert!(t0.elapsed() < Duration::from_millis(1000));
    stop.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
}

#[tokio::test]
async fn reconnects_with_backoff_and_rereads_a_rotated_token() {
    let (l, addr) = bind().await;
    let token = Arc::new(Mutex::new("old".to_string()));
    let api = Arc::new(MockApi::default());
    let (stop, h) = start(cfg(addr, Arc::clone(&token)), api);

    let mut ws = accept(&l).await;
    handshake(&mut ws, "old").await;
    // The daemon rotates the token: close 4401. The agent must re-read and authenticate again.
    *token.lock().unwrap() = "new".into();
    ws.send(Message::Close(Some(CloseFrame {
        code: CloseCode::Library(4401),
        reason: "token rotated".into(),
    })))
    .await
    .unwrap();
    drop(ws);
    let mut ws2 = tokio::time::timeout(Duration::from_secs(3), accept(&l))
        .await
        .expect("reconnect");
    handshake(&mut ws2, "new").await;

    // A plain connection drop is retried too.
    drop(ws2);
    let mut ws3 = tokio::time::timeout(Duration::from_secs(3), accept(&l))
        .await
        .expect("reconnect 2");
    handshake(&mut ws3, "new").await;
    stop.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
}

#[tokio::test]
async fn a_rejected_token_is_retried_with_the_current_token() {
    let (l, addr) = bind().await;
    let token = Arc::new(Mutex::new("wrong".to_string()));
    let (stop, h) = start(cfg(addr, Arc::clone(&token)), Arc::new(MockApi::default()));
    // The daemon closes without auth_ok.
    let mut ws = accept(&l).await;
    let first = next_text(&mut ws, 2000).await.unwrap();
    assert!(first.contains("wrong"));
    ws.close(None).await.unwrap();
    drop(ws);
    *token.lock().unwrap() = "right".into();
    let mut ws2 = tokio::time::timeout(Duration::from_secs(3), accept(&l))
        .await
        .expect("retry");
    handshake(&mut ws2, "right").await;
    stop.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
}

#[tokio::test]
async fn nothing_is_sent_before_auth_and_a_silent_daemon_times_out() {
    let (l, addr) = bind().await;
    let (stop, h) = start(
        cfg(addr, Arc::new(Mutex::new("tok".into()))),
        Arc::new(MockApi::default()),
    );
    let mut ws = accept(&l).await;
    let first = next_text(&mut ws, 2000).await.unwrap();
    assert!(first.contains("\"auth\""));
    // No auth_ok: the agent must not send hello / state, and gives up after the auth timeout.
    assert!(next_text(&mut ws, 500).await.is_none());
    let mut ws2 = tokio::time::timeout(Duration::from_secs(4), accept(&l))
        .await
        .expect("reconnect after timeout");
    let first = next_text(&mut ws2, 2000).await.unwrap();
    assert!(first.contains("\"auth\""));
    stop.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
}

#[tokio::test]
async fn being_replaced_by_a_newer_agent_waits_before_reconnecting() {
    let (l, addr) = bind().await;
    let mut c = cfg(addr, Arc::new(Mutex::new("tok".into())));
    c.replaced_backoff = Duration::from_millis(600);
    let (stop, h) = start(c, Arc::new(MockApi::default()));
    let mut ws = accept(&l).await;
    handshake(&mut ws, "tok").await;
    ws.send(Message::Close(Some(CloseFrame {
        code: CloseCode::Library(4409),
        reason: "replaced".into(),
    })))
    .await
    .unwrap();
    drop(ws);
    let t = std::time::Instant::now();
    let _ws2 = tokio::time::timeout(Duration::from_secs(4), accept(&l))
        .await
        .expect("reconnect");
    assert!(
        t.elapsed() >= Duration::from_millis(500),
        "must not reconnect at once: {:?}",
        t.elapsed()
    );
    stop.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
}

#[tokio::test]
async fn no_token_means_no_connection_attempt_but_the_loop_keeps_trying() {
    let (l, addr) = bind().await;
    let present = Arc::new(Mutex::new(Option::<String>::None));
    let p2 = Arc::clone(&present);
    let mut c = ClientConfig::new(
        addr.to_string(),
        Arc::new(move || p2.lock().unwrap().clone()),
    );
    c.backoff_min = Duration::from_millis(30);
    c.backoff_max = Duration::from_millis(60);
    let (stop, h) = start(c, Arc::new(MockApi::default()));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), l.accept())
            .await
            .is_err(),
        "no token, no connection"
    );
    *present.lock().unwrap() = Some("late".into());
    let mut ws = tokio::time::timeout(Duration::from_secs(3), accept(&l))
        .await
        .expect("connects once the token exists");
    handshake(&mut ws, "late").await;
    stop.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
}
