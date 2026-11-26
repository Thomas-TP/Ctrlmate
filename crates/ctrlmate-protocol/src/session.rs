//! Agent protocol (`/ipc/agent`): everything bound to the interactive user session
//! (architecture contract section 3).

use crate::{AllowlistedScript, CommandResult};
use serde::{Deserialize, Serialize};

/// Foreground window of the interactive session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundApp {
    pub process_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
}

/// One audio playback endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
}

/// Master audio state of the interactive session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioState {
    /// Master volume, 0-100.
    pub volume: f64,
    pub muted: bool,
    /// Id of the current default output, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_id: Option<String>,
    #[serde(default)]
    pub outputs: Vec<AudioOutput>,
}

/// Everything the agent observes in the user session. `None` means "not measurable",
/// never "false" or "50 %".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionState {
    #[serde(default)]
    pub foreground_app: Option<ForegroundApp>,
    #[serde(default)]
    pub gaming_mode: bool,
    /// Workstation locked (or secure desktop active).
    #[serde(default)]
    pub locked: bool,
    /// Seconds since the last keyboard / mouse input.
    #[serde(default)]
    pub idle_seconds: u64,
    #[serde(default)]
    pub mic_in_use: Option<bool>,
    #[serde(default)]
    pub webcam_in_use: Option<bool>,
    #[serde(default)]
    pub audio: Option<AudioState>,
}

/// Action the daemon asks the agent to perform in the user session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionAction {
    Lock,
    DisplayOff,
    Notify {
        title: String,
        body: String,
    },
    SetVolume {
        percent: f64,
    },
    SetMute {
        muted: bool,
    },
    SetAudioOutput {
        id: String,
    },
    /// The agent resolves `script_id` in the allowlist the daemon pushed; it never accepts a path.
    RunScript {
        script_id: String,
    },
}

/// Agent to daemon frames (after the first-frame `auth`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    Hello {
        agent_version: String,
        /// `windows` or `linux`.
        os: String,
        session_id: u32,
        user: String,
    },
    SessionState(SessionState),
    Response {
        id: String,
        ok: bool,
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

impl AgentMessage {
    /// Builds a `response` frame from a [`CommandResult`].
    pub fn response(id: impl Into<String>, result: CommandResult) -> Self {
        Self::Response {
            id: id.into(),
            ok: result.ok,
            code: result.code,
            message: result.message,
        }
    }
}

/// Daemon to agent frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonToAgent {
    /// Auth accepted; the agent should send `hello` and start streaming `session_state`.
    AuthOk,
    Request {
        id: String,
        action: SessionAction,
    },
    /// Current script allowlist (enabled scripts only). Re-sent on every change.
    Scripts {
        scripts: Vec<AllowlistedScript>,
    },
}

/// First frame of every IPC WebSocket (`/ipc/ws`, `/ipc/agent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcAuthFrame {
    Auth { token: String },
}

/// Seconds without a `session_state` after which the daemon treats the agent state as stale.
pub const AGENT_STATE_STALE_SECS: u64 = 15;
/// Seconds before an unanswered agent request fails with `agent_timeout`.
pub const AGENT_REQUEST_TIMEOUT_SECS: u64 = 10;
/// Seconds allowed for the first-frame IPC authentication.
pub const IPC_WS_AUTH_TIMEOUT_SECS: u64 = 5;
/// Idle seconds after which the session counts as idle for adaptive sampling.
pub const IDLE_THRESHOLD_SECS: u64 = 300;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_frames_match_contract() {
        let hello: AgentMessage = serde_json::from_value(json!({
            "type":"hello","agent_version":"0.6.0","os":"windows","session_id":2,"user":"leo"
        }))
        .unwrap();
        assert!(matches!(hello, AgentMessage::Hello { session_id: 2, .. }));

        let st: AgentMessage = serde_json::from_value(json!({
            "type":"session_state","gaming_mode":true,"locked":false,"idle_seconds":12,
            "foreground_app":{"process_name":"game.exe"},
            "mic_in_use":false,
            "audio":{"volume":30.0,"muted":false,"output_id":"a","outputs":[{"id":"a","name":"Speakers"}]}
        }))
        .unwrap();
        let AgentMessage::SessionState(s) = st else {
            panic!("expected session_state")
        };
        assert!(s.gaming_mode);
        assert_eq!(s.webcam_in_use, None);
        assert_eq!(s.audio.unwrap().outputs[0].name, "Speakers");

        let resp: AgentMessage = serde_json::from_value(
            json!({"type":"response","id":"7","ok":false,"code":"not_supported"}),
        )
        .unwrap();
        assert!(matches!(resp, AgentMessage::Response { ok: false, .. }));
    }

    #[test]
    fn daemon_frames_roundtrip() {
        let req = DaemonToAgent::Request {
            id: "1".into(),
            action: SessionAction::Notify {
                title: "t".into(),
                body: "b".into(),
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v,
            json!({"type":"request","id":"1","action":{"type":"notify","title":"t","body":"b"}})
        );
        assert_eq!(serde_json::from_value::<DaemonToAgent>(v).unwrap(), req);

        let auth: IpcAuthFrame = serde_json::from_str(r#"{"type":"auth","token":"abc"}"#).unwrap();
        assert_eq!(
            auth,
            IpcAuthFrame::Auth {
                token: "abc".into()
            }
        );
        assert!(serde_json::from_str::<IpcAuthFrame>(r#"{"type":"nope"}"#).is_err());
    }

    #[test]
    fn response_helper() {
        let m = AgentMessage::response("9", CommandResult::err(crate::ResultCode::NotSupported));
        let v = serde_json::to_value(m).unwrap();
        assert_eq!(v["code"], "not_supported");
        assert_eq!(v["ok"], false);
    }
}
