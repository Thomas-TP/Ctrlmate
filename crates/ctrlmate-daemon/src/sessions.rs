//! Session backends: everything bound to the interactive user session goes through a
//! [`SessionBackend`].
//!
//! * [`AgentBackend`] forwards to the connected `ctrlmate-agent` (timeouts, `no_agent`).
//! * [`InProcessBackend`] runs the same features inside the daemon through `ctrlmate_hal`; it is
//!   only created when the daemon itself runs in an interactive session (dev / `--foreground`).
//! * [`Sessions`] picks the agent when one is connected, the in-process fallback otherwise, and
//!   answers `no_agent` when there is neither. There is never a fake success.
//!
//! Custom scripts run **only** through the agent.

use crate::scripts::enabled_subset;
use chrono::Utc;
use ctrlmate_hal::session::SessionRuntime;
use ctrlmate_protocol::{
    AGENT_STATE_STALE_SECS, AllowlistedScript, CommandResult, DaemonToAgent, EntityState,
    EntityValue, ResultCode, SessionAction, SessionState,
};
use futures_util::future::BoxFuture;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub trait SessionBackend: Send + Sync {
    /// Runs `action` in the user session.
    fn execute(&self, action: SessionAction) -> BoxFuture<'_, CommandResult>;
    /// Latest fresh session state.
    fn state(&self) -> Option<SessionState>;
    /// Whether `execute` can currently reach a user session.
    fn is_available(&self) -> bool;
}

// ---------------------------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AgentHello {
    pub agent_version: String,
    pub session_id: u32,
}

struct AgentConn {
    conn_id: u64,
    tx: mpsc::Sender<DaemonToAgent>,
    kick: watch::Sender<bool>,
    hello: Option<AgentHello>,
}

#[derive(Default)]
struct AgentInner {
    conn: Option<AgentConn>,
    state: Option<(SessionState, Instant)>,
    pending: HashMap<String, oneshot::Sender<CommandResult>>,
    scripts: Vec<AllowlistedScript>,
}

/// Handle returned by [`AgentBackend::attach`].
pub struct AgentAttachment {
    pub conn_id: u64,
    /// Becomes `true` when a newer agent replaced this connection.
    pub kicked: watch::Receiver<bool>,
    /// Frames to send to the agent.
    pub outgoing: mpsc::Receiver<DaemonToAgent>,
}

pub struct AgentBackend {
    inner: Mutex<AgentInner>,
    next_conn: AtomicU64,
    next_req: AtomicU64,
    timeout: Duration,
}

impl AgentBackend {
    pub fn new(timeout: Duration) -> Self {
        Self {
            inner: Mutex::new(AgentInner::default()),
            next_conn: AtomicU64::new(1),
            next_req: AtomicU64::new(1),
            timeout,
        }
    }

    /// Registers a new agent connection, replacing (and kicking) any previous one. The newest
    /// agent wins, e.g. after a fast user switch.
    pub fn attach(&self) -> AgentAttachment {
        let (tx, outgoing) = mpsc::channel(32);
        let (kick, kicked) = watch::channel(false);
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        {
            let mut g = lock(&self.inner);
            if let Some(old) = g.conn.take() {
                let _ = old.kick.send(true);
            }
            // Pending requests of the replaced agent can never be answered.
            g.pending.clear();
            g.state = None;
            g.conn = Some(AgentConn {
                conn_id,
                tx,
                kick,
                hello: None,
            });
        }
        AgentAttachment {
            conn_id,
            kicked,
            outgoing,
        }
    }

    /// Removes the connection if it is still the current one.
    pub fn detach(&self, conn_id: u64) {
        let mut g = lock(&self.inner);
        if g.conn.as_ref().is_some_and(|c| c.conn_id == conn_id) {
            g.conn = None;
            g.state = None;
            g.pending.clear();
        }
    }

    pub fn set_hello(&self, conn_id: u64, hello: AgentHello) {
        let mut g = lock(&self.inner);
        if let Some(c) = g.conn.as_mut().filter(|c| c.conn_id == conn_id) {
            c.hello = Some(hello);
        }
    }

    pub fn hello(&self) -> Option<AgentHello> {
        lock(&self.inner)
            .conn
            .as_ref()
            .and_then(|c| c.hello.clone())
    }

    /// Stores the latest state; returns `false` when `conn_id` is not the current connection.
    pub fn set_state(&self, conn_id: u64, state: SessionState) -> bool {
        let mut g = lock(&self.inner);
        if g.conn.as_ref().is_none_or(|c| c.conn_id != conn_id) {
            return false;
        }
        g.state = Some((state, Instant::now()));
        true
    }

    /// Delivers an agent `response` to the waiting request.
    pub fn resolve(&self, conn_id: u64, id: &str, result: CommandResult) {
        let tx = {
            let mut g = lock(&self.inner);
            if g.conn.as_ref().is_none_or(|c| c.conn_id != conn_id) {
                return;
            }
            g.pending.remove(id)
        };
        if let Some(tx) = tx {
            let _ = tx.send(result);
        }
    }

    /// Updates the allowlist and pushes it to the connected agent (enabled scripts only).
    pub fn push_scripts(&self, all: &[AllowlistedScript]) {
        let subset = enabled_subset(all);
        let tx = {
            let mut g = lock(&self.inner);
            g.scripts.clone_from(&subset);
            g.conn.as_ref().map(|c| c.tx.clone())
        };
        if let Some(tx) = tx {
            let _ = tx.try_send(DaemonToAgent::Scripts { scripts: subset });
        }
    }

    /// Current allowlist snapshot (sent right after authentication).
    pub fn scripts(&self) -> Vec<AllowlistedScript> {
        lock(&self.inner).scripts.clone()
    }

    fn connected(&self) -> bool {
        lock(&self.inner).conn.is_some()
    }
}

impl SessionBackend for AgentBackend {
    fn execute(&self, action: SessionAction) -> BoxFuture<'_, CommandResult> {
        Box::pin(async move {
            let id = self.next_req.fetch_add(1, Ordering::Relaxed).to_string();
            let (rtx, rrx) = oneshot::channel();
            let (tx, conn_id) = {
                let mut g = lock(&self.inner);
                let Some(c) = g.conn.as_ref() else {
                    return CommandResult::err(ResultCode::NoAgent);
                };
                let (tx, conn_id) = (c.tx.clone(), c.conn_id);
                g.pending.insert(id.clone(), rtx);
                (tx, conn_id)
            };
            if tx
                .send(DaemonToAgent::Request {
                    id: id.clone(),
                    action,
                })
                .await
                .is_err()
            {
                self.drop_pending(conn_id, &id);
                return CommandResult::err(ResultCode::NoAgent);
            }
            match tokio::time::timeout(self.timeout, rrx).await {
                Ok(Ok(result)) => result,
                // The sender was dropped: the agent disconnected mid-request.
                Ok(Err(_)) => CommandResult::err(ResultCode::NoAgent),
                Err(_) => {
                    self.drop_pending(conn_id, &id);
                    CommandResult::err(ResultCode::AgentTimeout)
                }
            }
        })
    }

    fn state(&self) -> Option<SessionState> {
        let g = lock(&self.inner);
        g.state
            .as_ref()
            .filter(|(_, at)| at.elapsed() <= Duration::from_secs(AGENT_STATE_STALE_SECS))
            .map(|(s, _)| s.clone())
    }

    fn is_available(&self) -> bool {
        self.connected()
    }
}

impl AgentBackend {
    fn drop_pending(&self, conn_id: u64, id: &str) {
        let mut g = lock(&self.inner);
        if g.conn.as_ref().is_some_and(|c| c.conn_id == conn_id) {
            g.pending.remove(id);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// In-process fallback (dev / interactive daemon only)
// ---------------------------------------------------------------------------------------------

pub struct InProcessBackend {
    runtime: Arc<SessionRuntime>,
    state: Mutex<Option<(SessionState, Instant)>>,
}

impl Default for InProcessBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessBackend {
    pub fn new() -> Self {
        Self {
            runtime: Arc::new(SessionRuntime::new()),
            state: Mutex::new(None),
        }
    }

    /// Samples the session through `ctrlmate_hal::session` (COM / Win32 calls, so on a blocking
    /// thread). Every value is `None` / absent when it cannot be measured; nothing is invented.
    pub async fn refresh(&self) {
        let rt = Arc::clone(&self.runtime);
        if let Ok(state) = tokio::task::spawn_blocking(move || rt.sample()).await {
            *lock(&self.state) = Some((state, Instant::now()));
        }
    }
}

impl SessionBackend for InProcessBackend {
    fn execute(&self, action: SessionAction) -> BoxFuture<'_, CommandResult> {
        Box::pin(async move {
            // Scripts run only through the agent, in the user's session.
            if matches!(action, SessionAction::RunScript { .. }) {
                return CommandResult::err(ResultCode::NotSupported);
            }
            let rt = Arc::clone(&self.runtime);
            tokio::task::spawn_blocking(move || rt.execute(&action))
                .await
                .unwrap_or_else(|e| CommandResult::err_with(ResultCode::Internal, e.to_string()))
        })
    }

    fn state(&self) -> Option<SessionState> {
        lock(&self.state)
            .as_ref()
            .filter(|(_, at)| at.elapsed() <= Duration::from_secs(AGENT_STATE_STALE_SECS))
            .map(|(s, _)| s.clone())
    }

    fn is_available(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------------------------

pub struct Sessions {
    pub agent: Arc<AgentBackend>,
    pub fallback: Option<Arc<InProcessBackend>>,
}

impl Sessions {
    pub fn new(agent: Arc<AgentBackend>, fallback: Option<Arc<InProcessBackend>>) -> Self {
        Self { agent, fallback }
    }

    pub fn agent_connected(&self) -> bool {
        self.agent.is_available()
    }

    pub fn in_process_active(&self) -> bool {
        self.fallback.is_some() && !self.agent.is_available()
    }
}

impl SessionBackend for Sessions {
    fn execute(&self, action: SessionAction) -> BoxFuture<'_, CommandResult> {
        if self.agent.is_available() {
            self.agent.execute(action)
        } else if let Some(fb) = &self.fallback {
            fb.execute(action)
        } else {
            Box::pin(async { CommandResult::err(ResultCode::NoAgent) })
        }
    }

    fn state(&self) -> Option<SessionState> {
        if self.agent.is_available() {
            self.agent.state()
        } else {
            self.fallback.as_ref().and_then(|f| f.state())
        }
    }

    fn is_available(&self) -> bool {
        self.agent.is_available() || self.fallback.is_some()
    }
}

// ---------------------------------------------------------------------------------------------
// Overlay of session data on the hardware states
// ---------------------------------------------------------------------------------------------

/// Entity ids that describe the interactive session. In session 0 the HAL can only fabricate
/// values for them, so the daemon always overwrites them from the `SessionState` (or marks them
/// unavailable when there is none).
pub const SESSION_ENTITIES: [&str; 7] = [
    "active_application",
    "gaming_mode",
    "microphone_in_use",
    "webcam_in_use",
    "master_volume",
    "audio_mute",
    "audio_output",
];

fn put(
    states: &mut Vec<EntityState>,
    id: &str,
    value: EntityValue,
    attrs: BTreeMap<String, serde_json::Value>,
) {
    let now = Utc::now();
    if let Some(s) = states.iter_mut().find(|s| s.id == id) {
        s.value = value;
        s.attributes = attrs;
        s.updated_at = now;
    } else {
        states.push(EntityState {
            id: id.into(),
            value,
            attributes: attrs,
            updated_at: now,
        });
    }
}

fn opt_bool(v: Option<bool>) -> EntityValue {
    v.map_or(EntityValue::Unavailable, EntityValue::Bool)
}

/// Replaces the session-bound entities in `states` with the real session data.
pub fn overlay_session(states: &mut Vec<EntityState>, session: Option<&SessionState>) {
    let none = BTreeMap::new;
    let Some(s) = session else {
        for id in SESSION_ENTITIES {
            put(states, id, EntityValue::Unavailable, none());
        }
        return;
    };

    match &s.foreground_app {
        Some(app) => {
            let mut attrs = BTreeMap::new();
            attrs.insert("process_name".into(), serde_json::json!(app.process_name));
            if let Some(t) = &app.window_title {
                attrs.insert("window_title".into(), serde_json::json!(t));
            }
            put(
                states,
                "active_application",
                EntityValue::Text(app.process_name.clone()),
                attrs,
            );
        }
        None => put(
            states,
            "active_application",
            EntityValue::Unavailable,
            none(),
        ),
    }
    put(
        states,
        "gaming_mode",
        EntityValue::Bool(s.gaming_mode),
        none(),
    );
    put(states, "microphone_in_use", opt_bool(s.mic_in_use), none());
    put(states, "webcam_in_use", opt_bool(s.webcam_in_use), none());

    match &s.audio {
        Some(a) => {
            put(
                states,
                "master_volume",
                EntityValue::Float(a.volume.clamp(0.0, 100.0)),
                none(),
            );
            put(states, "audio_mute", EntityValue::Bool(a.muted), none());
            let current = a
                .output_id
                .as_ref()
                .and_then(|id| a.outputs.iter().find(|o| &o.id == id))
                .map(|o| o.name.clone());
            put(
                states,
                "audio_output",
                current.map_or(EntityValue::Unavailable, EntityValue::Text),
                none(),
            );
        }
        None => {
            for id in ["master_volume", "audio_mute", "audio_output"] {
                put(states, id, EntityValue::Unavailable, none());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctrlmate_protocol::{AgentMessage, AudioOutput, AudioState, ForegroundApp};

    fn backend() -> Arc<AgentBackend> {
        Arc::new(AgentBackend::new(Duration::from_millis(150)))
    }

    #[tokio::test]
    async fn no_agent_means_no_agent_never_fake_success() {
        let b = backend();
        let r = b.execute(SessionAction::Lock).await;
        assert!(!r.ok);
        assert_eq!(r.code, "no_agent");
        let sessions = Sessions::new(b, None);
        assert!(!sessions.is_available());
        assert_eq!(
            sessions.execute(SessionAction::DisplayOff).await.code,
            "no_agent"
        );
    }

    #[tokio::test]
    async fn request_response_roundtrip() {
        let b = backend();
        let mut att = b.attach();
        let b2 = Arc::clone(&b);
        let conn = att.conn_id;
        tokio::spawn(async move {
            while let Some(DaemonToAgent::Request { id, action }) = att.outgoing.recv().await {
                assert_eq!(action, SessionAction::SetMute { muted: true });
                b2.resolve(conn, &id, CommandResult::ok_with("muted"));
            }
        });
        let r = b.execute(SessionAction::SetMute { muted: true }).await;
        assert!(r.ok, "{r:?}");
        assert_eq!(r.message.as_deref(), Some("muted"));
    }

    #[tokio::test]
    async fn unanswered_request_times_out_with_code() {
        let b = backend();
        let _att = b.attach();
        let r = b.execute(SessionAction::Lock).await;
        assert_eq!(r.code, "agent_timeout");
        // The pending slot was cleaned up.
        assert!(lock(&b.inner).pending.is_empty());
    }

    #[tokio::test]
    async fn disconnect_mid_request_is_no_agent() {
        let b = backend();
        let att = b.attach();
        let b2 = Arc::clone(&b);
        let conn = att.conn_id;
        let t = tokio::spawn(async move { b2.execute(SessionAction::Lock).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        b.detach(conn);
        assert_eq!(t.await.unwrap().code, "no_agent");
        assert!(!b.is_available());
        drop(att);
    }

    #[tokio::test]
    async fn newer_agent_replaces_and_kicks_the_old_one() {
        let b = backend();
        let old = b.attach();
        let new = b.attach();
        assert!(*old.kicked.borrow());
        assert!(!*new.kicked.borrow());
        // A stale connection cannot write state or detach the new one.
        assert!(!b.set_state(old.conn_id, SessionState::default()));
        b.detach(old.conn_id);
        assert!(b.is_available());
        assert!(b.set_state(new.conn_id, SessionState::default()));
    }

    #[tokio::test]
    async fn state_goes_stale() {
        let b = backend();
        let att = b.attach();
        assert!(b.state().is_none());
        b.set_state(
            att.conn_id,
            SessionState {
                gaming_mode: true,
                ..SessionState::default()
            },
        );
        assert!(b.state().is_some_and(|s| s.gaming_mode));
        // Age the state artificially.
        {
            let mut g = lock(&b.inner);
            if let Some((_, at)) = g.state.as_mut() {
                *at = Instant::now()
                    .checked_sub(Duration::from_secs(AGENT_STATE_STALE_SECS + 1))
                    .unwrap();
            }
        }
        assert!(b.state().is_none());
    }

    #[tokio::test]
    async fn scripts_are_pushed_enabled_only() {
        let b = backend();
        let mut att = b.attach();
        let mk = |id: &str, enabled| AllowlistedScript {
            id: id.into(),
            name: id.into(),
            executable: "/x".into(),
            args: vec![],
            working_dir: None,
            enabled,
        };
        b.push_scripts(&[mk("on", true), mk("off", false)]);
        match att.outgoing.recv().await {
            Some(DaemonToAgent::Scripts { scripts }) => {
                assert_eq!(scripts.len(), 1);
                assert_eq!(scripts[0].id, "on");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(b.scripts().len(), 1);
    }

    #[tokio::test]
    async fn in_process_never_runs_scripts_and_router_prefers_agent() {
        let fb = Arc::new(InProcessBackend::new());
        let r = fb
            .execute(SessionAction::RunScript {
                script_id: "a".into(),
            })
            .await;
        assert_eq!(r.code, "not_supported");
        let b = backend();
        let sessions = Sessions::new(Arc::clone(&b), Some(fb));
        assert!(sessions.in_process_active());
        let att = b.attach();
        assert!(!sessions.in_process_active());
        assert!(sessions.agent_connected());
        drop(att);
    }

    #[test]
    fn overlay_without_session_marks_everything_unavailable() {
        let mut states = vec![crate::hw::fake::state(
            "master_volume",
            EntityValue::Float(50.0),
        )];
        overlay_session(&mut states, None);
        for id in SESSION_ENTITIES {
            let s = states.iter().find(|s| s.id == id).expect(id);
            assert_eq!(s.value, EntityValue::Unavailable, "{id}");
        }
    }

    #[test]
    fn overlay_with_session_uses_real_values() {
        let mut states = vec![crate::hw::fake::state(
            "gaming_mode",
            EntityValue::Bool(false),
        )];
        let s = SessionState {
            foreground_app: Some(ForegroundApp {
                process_name: "game.exe".into(),
                window_title: Some("Game".into()),
            }),
            gaming_mode: true,
            mic_in_use: Some(true),
            webcam_in_use: None,
            audio: Some(AudioState {
                volume: 140.0,
                muted: true,
                output_id: Some("b".into()),
                outputs: vec![
                    AudioOutput {
                        id: "a".into(),
                        name: "Speakers".into(),
                    },
                    AudioOutput {
                        id: "b".into(),
                        name: "Headset".into(),
                    },
                ],
            }),
            ..SessionState::default()
        };
        overlay_session(&mut states, Some(&s));
        let get = |id: &str| states.iter().find(|s| s.id == id).map(|s| s.value.clone());
        assert_eq!(get("gaming_mode"), Some(EntityValue::Bool(true)));
        assert_eq!(
            get("active_application"),
            Some(EntityValue::Text("game.exe".into()))
        );
        assert_eq!(get("microphone_in_use"), Some(EntityValue::Bool(true)));
        assert_eq!(get("webcam_in_use"), Some(EntityValue::Unavailable));
        assert_eq!(get("master_volume"), Some(EntityValue::Float(100.0)));
        assert_eq!(
            get("audio_output"),
            Some(EntityValue::Text("Headset".into()))
        );
    }

    #[test]
    fn agent_message_response_maps_into_result() {
        let m = AgentMessage::response("1", CommandResult::err_with(ResultCode::NotSupported, "x"));
        let AgentMessage::Response {
            ok, code, message, ..
        } = m
        else {
            panic!()
        };
        assert!(!ok);
        assert_eq!(code, "not_supported");
        assert_eq!(message.as_deref(), Some("x"));
    }
}
