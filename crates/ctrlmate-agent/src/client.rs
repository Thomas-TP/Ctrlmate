//! The agent's connection to the daemon (`/ipc/agent`, architecture contract section 3).
//!
//! Protocol: the first frame is `{"type":"auth","token":...}`; the daemon answers `auth_ok` (then
//! the `scripts` allowlist). The agent sends `hello`, then `session_state` on every significant
//! change and at least every [`ClientConfig::heartbeat`] (5 s), and answers each `request` with a
//! `response`. A dropped connection is retried with an exponential backoff; a rotated token (close
//! code 4401) is re-read and retried at once; a replaced agent (4409) waits much longer so two
//! sessions do not fight over the connection.

use anyhow::{Context, Result, anyhow, bail};
use ctrlmate_hal::session::SessionRuntime;
use ctrlmate_protocol::{
    AgentMessage, AllowlistedScript, CommandResult, DaemonToAgent, IDLE_THRESHOLD_SECS,
    IpcAuthFrame, SessionAction, SessionState,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// WebSocket close code of a rotated IPC token.
pub const CLOSE_TOKEN_ROTATED: u16 = 4401;
/// WebSocket close code sent to an agent replaced by a newer one.
pub const CLOSE_REPLACED: u16 = 4409;

/// What the client needs from the session layer (the real one is `SessionRuntime`).
pub trait SessionApi: Send + Sync + 'static {
    /// Observes the session (blocking).
    fn sample(&self) -> SessionState;
    /// Performs an action (blocking).
    fn execute(&self, action: &SessionAction) -> CommandResult;
    /// Replaces the script allowlist pushed by the daemon.
    fn set_scripts(&self, scripts: Vec<AllowlistedScript>);
}

impl SessionApi for SessionRuntime {
    fn sample(&self) -> SessionState {
        SessionRuntime::sample(self)
    }
    fn execute(&self, action: &SessionAction) -> CommandResult {
        SessionRuntime::execute(self, action)
    }
    fn set_scripts(&self, scripts: Vec<AllowlistedScript>) {
        SessionRuntime::set_scripts(self, scripts);
    }
}

/// Facts sent in `hello`.
#[derive(Debug, Clone)]
pub struct Identity {
    pub os: String,
    pub session_id: u32,
    pub user: String,
}

impl Identity {
    pub fn detect() -> Self {
        Self {
            os: if cfg!(windows) { "windows" } else { "linux" }.into(),
            session_id: session_id(),
            user: std::env::var("USERNAME")
                .or_else(|_| std::env::var("USER"))
                .unwrap_or_default(),
        }
    }
}

#[cfg(target_os = "windows")]
fn session_id() -> u32 {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    let mut sid = 0u32;
    // SAFETY: valid out-pointer.
    unsafe {
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut sid);
    }
    sid
}

#[cfg(not(target_os = "windows"))]
fn session_id() -> u32 {
    std::env::var("XDG_SESSION_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

type TokenFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

#[derive(Clone)]
pub struct ClientConfig {
    /// `host:port` of the daemon's loopback IPC listener.
    pub endpoint: String,
    /// Called at every connection attempt so a rotated token is picked up.
    pub token: TokenFn,
    pub identity: Identity,
    /// How often the session is sampled for changes.
    pub sample_interval: Duration,
    /// Maximum time between two `session_state` frames.
    pub heartbeat: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Wait after being replaced by a newer agent.
    pub replaced_backoff: Duration,
    pub auth_timeout: Duration,
}

impl ClientConfig {
    pub fn new(endpoint: String, token: TokenFn) -> Self {
        Self {
            endpoint,
            token,
            identity: Identity::detect(),
            sample_interval: Duration::from_secs(2),
            heartbeat: Duration::from_secs(5),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            replaced_backoff: Duration::from_secs(300),
            auth_timeout: Duration::from_secs(5),
        }
    }
}

/// `true` when `new` differs from `old` in a way the daemon cares about right away. `idle_seconds`
/// grows continuously, so only the idle *tier* (the 5 minute threshold) counts as a change.
pub fn significant_change(old: &SessionState, new: &SessionState) -> bool {
    let tier = |s: &SessionState| s.idle_seconds >= IDLE_THRESHOLD_SECS;
    let mut a = old.clone();
    let mut b = new.clone();
    a.idle_seconds = 0;
    b.idle_seconds = 0;
    a != b || tier(old) != tier(new)
}

enum Outcome {
    Shutdown,
    TokenRotated,
    Replaced,
    Ended { connected_for: Duration },
    Failed(anyhow::Error),
}

/// Runs the agent connection until `shutdown` turns `true`.
pub async fn run(cfg: ClientConfig, api: Arc<dyn SessionApi>, mut shutdown: watch::Receiver<bool>) {
    let mut backoff = cfg.backoff_min;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let wait = match one_session(&cfg, &api, &mut shutdown).await {
            Outcome::Shutdown => return,
            Outcome::TokenRotated => {
                info!("IPC token rotated: reconnecting with the new token");
                backoff = cfg.backoff_min;
                Duration::from_millis(200)
            }
            Outcome::Replaced => {
                info!(
                    "Replaced by a newer agent; waiting {:?} before reconnecting",
                    cfg.replaced_backoff
                );
                backoff = cfg.backoff_min;
                cfg.replaced_backoff
            }
            Outcome::Ended { connected_for } => {
                if connected_for > Duration::from_secs(10) {
                    backoff = cfg.backoff_min;
                }
                let w = backoff;
                backoff = (backoff * 2).min(cfg.backoff_max);
                info!("Daemon connection ended after {connected_for:?}; retrying in {w:?}");
                w
            }
            Outcome::Failed(e) => {
                let w = backoff;
                backoff = (backoff * 2).min(cfg.backoff_max);
                debug!("Daemon connection failed: {e:#}; retrying in {w:?}");
                w
            }
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {}
        }
    }
}

fn frame<T: serde::Serialize>(v: &T) -> Result<Message> {
    Ok(Message::Text(serde_json::to_string(v)?.into()))
}

async fn one_session(
    cfg: &ClientConfig,
    api: &Arc<dyn SessionApi>,
    shutdown: &mut watch::Receiver<bool>,
) -> Outcome {
    match session_inner(cfg, api, shutdown).await {
        Ok(o) => o,
        Err(e) => Outcome::Failed(e),
    }
}

async fn session_inner(
    cfg: &ClientConfig,
    api: &Arc<dyn SessionApi>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Outcome> {
    let token =
        (cfg.token)().ok_or_else(|| anyhow!("no IPC token found (env, user dir, system dir)"))?;
    let url = format!("ws://{}/ipc/agent", cfg.endpoint);
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .map_err(|_| anyhow!("connect timed out"))?
    .with_context(|| format!("cannot connect to {url}"))?;

    // The very first frame is the authentication.
    ws.send(frame(&IpcAuthFrame::Auth { token })?).await?;
    let authed = tokio::time::timeout(cfg.auth_timeout, async {
        while let Some(m) = ws.next().await {
            match m? {
                Message::Text(t) => match serde_json::from_str::<DaemonToAgent>(&t) {
                    Ok(DaemonToAgent::AuthOk) => return Ok(true),
                    Ok(other) => bail!("unexpected frame before auth_ok: {other:?}"),
                    Err(e) => bail!("malformed frame before auth_ok: {e}"),
                },
                Message::Close(_) => return Ok(false),
                _ => {}
            }
        }
        Ok(false)
    })
    .await
    .map_err(|_| anyhow!("auth_ok not received in time"))??;
    if !authed {
        bail!("the daemon rejected the IPC token");
    }

    let started = Instant::now();
    info!("Connected to the daemon at {}", cfg.endpoint);
    ws.send(frame(&AgentMessage::Hello {
        agent_version: crate::AGENT_VERSION.into(),
        os: cfg.identity.os.clone(),
        session_id: cfg.identity.session_id,
        user: cfg.identity.user.clone(),
    })?)
    .await?;

    let mut sampling: JoinSet<SessionState> = JoinSet::new();
    let mut executing: JoinSet<(String, CommandResult)> = JoinSet::new();
    let mut last_sent: Option<(SessionState, Instant)> = None;
    let mut tick = tokio::time::interval(cfg.sample_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut resample = true; // sample right away

    loop {
        if resample && sampling.is_empty() {
            resample = false;
            let a = Arc::clone(api);
            sampling.spawn_blocking(move || a.sample());
        }
        tokio::select! {
            _ = shutdown.changed() => {
                let _ = ws.close(None).await;
                return Ok(Outcome::Shutdown);
            }
            _ = tick.tick() => resample = true,
            Some(done) = sampling.join_next() => {
                let Ok(state) = done else { continue };
                let due = last_sent.as_ref().is_none_or(|(prev, at)| {
                    at.elapsed() >= cfg.heartbeat || significant_change(prev, &state)
                });
                if due {
                    ws.send(frame(&AgentMessage::SessionState(state.clone()))?).await?;
                    last_sent = Some((state, Instant::now()));
                }
            }
            Some(done) = executing.join_next() => {
                let (id, result) = match done {
                    Ok(v) => v,
                    Err(e) => (String::new(), CommandResult::err_with(ctrlmate_protocol::ResultCode::Internal, e.to_string())),
                };
                if !id.is_empty() {
                    ws.send(frame(&AgentMessage::response(id, result))?).await?;
                }
                // The action probably changed the state (volume, mute, ...): report it soon.
                resample = true;
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<DaemonToAgent>(&t) {
                    Ok(DaemonToAgent::Scripts { scripts }) => {
                        debug!("Script allowlist updated ({} scripts)", scripts.len());
                        api.set_scripts(scripts);
                    }
                    Ok(DaemonToAgent::Request { id, action }) => {
                        let a = Arc::clone(api);
                        executing.spawn_blocking(move || {
                            let r = a.execute(&action);
                            (id, r)
                        });
                    }
                    Ok(DaemonToAgent::AuthOk) => {}
                    Err(e) => warn!("Ignoring malformed daemon frame: {e}"),
                },
                Some(Ok(Message::Close(cf))) => {
                    let code = cf.as_ref().map(|c| u16::from(c.code));
                    return Ok(match code {
                        Some(CLOSE_TOKEN_ROTATED) => Outcome::TokenRotated,
                        Some(CLOSE_REPLACED) => Outcome::Replaced,
                        _ => Outcome::Ended { connected_for: started.elapsed() },
                    });
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    debug!("WebSocket error: {e}");
                    return Ok(Outcome::Ended { connected_for: started.elapsed() });
                }
                None => return Ok(Outcome::Ended { connected_for: started.elapsed() }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctrlmate_protocol::{AudioState, ForegroundApp};

    #[test]
    fn idle_growth_is_not_a_change_but_the_idle_tier_is() {
        let base = SessionState {
            idle_seconds: 10,
            ..SessionState::default()
        };
        let mut later = base.clone();
        later.idle_seconds = 14;
        assert!(!significant_change(&base, &later));
        later.idle_seconds = IDLE_THRESHOLD_SECS;
        assert!(significant_change(&base, &later), "became idle");
        let idle = later.clone();
        let mut back = idle.clone();
        back.idle_seconds = 0;
        assert!(significant_change(&idle, &back), "user came back");
    }

    #[test]
    fn real_changes_are_significant() {
        let a = SessionState::default();
        let mut b = a.clone();
        b.gaming_mode = true;
        assert!(significant_change(&a, &b));
        let mut c = a.clone();
        c.foreground_app = Some(ForegroundApp {
            process_name: "x.exe".into(),
            window_title: None,
        });
        assert!(significant_change(&a, &c));
        let mut d = a.clone();
        d.audio = Some(AudioState {
            volume: 10.0,
            muted: false,
            output_id: None,
            outputs: vec![],
        });
        assert!(significant_change(&a, &d));
        assert!(!significant_change(&a, &a.clone()));
    }
}
