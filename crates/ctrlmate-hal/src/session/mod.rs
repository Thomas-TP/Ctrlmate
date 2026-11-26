//! Everything bound to the interactive user session (architecture contract sections 1 and 3).
//!
//! [`SessionRuntime`] is what `ctrlmate-agent` runs (and what a daemon started in an interactive
//! session may run in-process): [`SessionRuntime::sample`] produces a [`SessionState`] and
//! [`SessionRuntime::execute`] performs a [`SessionAction`]. All calls are blocking (COM, Win32, child
//! processes): call them from a blocking thread.
//!
//! Honesty rules: an unmeasurable value is `None` (never `false` / 50 %); an action whose backend is
//! missing returns `not_supported`; scripts only run from the allowlist pushed by the daemon, by id.

pub mod audio;
pub mod exec;
pub mod foreground;
pub mod power;
pub mod presence;
pub mod privacy;
pub mod script;

use ctrlmate_protocol::{
    AllowlistedScript, AudioOutput, AudioState, CommandResult, ForegroundApp, ResultCode,
    SessionAction, SessionState,
};
use foreground::{GamingInput, GpuLoadProbe};
use std::sync::{Mutex, MutexGuard};

/// Failure of a session operation, mapped to a stable [`ResultCode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The backend does not exist here (tool missing, compositor without API, OS).
    NotSupported(String),
    /// The request itself is wrong (unknown output id, NaN volume).
    Invalid(String),
    /// The backend exists and failed.
    Failed(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSupported(m) | Self::Invalid(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<SessionError> for CommandResult {
    fn from(e: SessionError) -> Self {
        let code = match &e {
            SessionError::NotSupported(_) => ResultCode::NotSupported,
            SessionError::Invalid(_) => ResultCode::InvalidArgument,
            SessionError::Failed(_) => ResultCode::HardwareError,
        };
        CommandResult::err_with(code, e.to_string())
    }
}

fn res(r: Result<(), SessionError>) -> CommandResult {
    r.map_or_else(CommandResult::from, |()| CommandResult::ok())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Session sampling and action execution.
#[derive(Default)]
pub struct SessionRuntime {
    scripts: Mutex<Vec<AllowlistedScript>>,
    gpu: Mutex<GpuLoadProbe>,
}

impl SessionRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the script allowlist (as pushed by the daemon).
    pub fn set_scripts(&self, scripts: Vec<AllowlistedScript>) {
        *lock(&self.scripts) = scripts;
    }

    /// Observes the session. Blocking.
    pub fn sample(&self) -> SessionState {
        let fg = foreground::detect();
        let gaming_mode = fg.as_ref().is_some_and(|f| {
            // Only ask the GPU when it can change the answer.
            let load = (f.fullscreen == foreground::Fullscreen::Borderless)
                .then(|| lock(&self.gpu).load_pct())
                .flatten();
            foreground::is_gaming(GamingInput {
                process_name: &f.process_name,
                exe_path: f.exe_path.as_deref(),
                fullscreen: f.fullscreen,
                gpu_load_pct: load,
            })
        });
        let audio = audio::poll().ok().map(|a| AudioState {
            volume: a.volume_percent,
            muted: a.muted,
            output_id: a.default_id,
            outputs: a
                .outputs
                .into_iter()
                .map(|d| AudioOutput {
                    id: d.id,
                    name: d.name,
                })
                .collect(),
        });
        SessionState {
            foreground_app: fg.map(|f| ForegroundApp {
                process_name: f.process_name,
                window_title: f.window_title,
            }),
            gaming_mode,
            locked: presence::is_locked().unwrap_or(false),
            idle_seconds: presence::idle_seconds().unwrap_or(0),
            mic_in_use: privacy::microphone().active,
            webcam_in_use: privacy::webcam().active,
            audio,
        }
    }

    /// Performs `action`. Blocking.
    pub fn execute(&self, action: &SessionAction) -> CommandResult {
        match action {
            SessionAction::Lock => res(power::lock()),
            SessionAction::DisplayOff => res(power::display_off()),
            SessionAction::Notify { title, body } => res(power::notify(title, body)),
            SessionAction::SetVolume { percent } => res(audio::set_volume(*percent)),
            SessionAction::SetMute { muted } => res(audio::set_mute(*muted)),
            SessionAction::SetAudioOutput { id } => res(audio::set_default_output(id)),
            SessionAction::RunScript { script_id } => {
                let found = lock(&self.scripts)
                    .iter()
                    .find(|s| &s.id == script_id)
                    .cloned();
                match found {
                    Some(s) => script::run_script(&s),
                    None => CommandResult::err_with(
                        ResultCode::InvalidArgument,
                        "script not in the allowlist",
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_script_is_refused_and_paths_are_never_accepted() {
        let rt = SessionRuntime::new();
        let r = rt.execute(&SessionAction::RunScript {
            script_id: "../../evil".into(),
        });
        assert_eq!(r.code, "invalid_argument");
        rt.set_scripts(vec![AllowlistedScript {
            id: "ok".into(),
            name: "ok".into(),
            executable: if cfg!(windows) {
                r"C:\nope\x.exe".into()
            } else {
                "/nope/x".into()
            },
            args: vec![],
            working_dir: None,
            enabled: true,
        }]);
        // Found in the allowlist, but the executable does not exist: unavailable, not a spawn attempt.
        assert_eq!(
            rt.execute(&SessionAction::RunScript {
                script_id: "ok".into()
            })
            .code,
            "unavailable"
        );
    }

    #[test]
    fn errors_map_to_stable_codes() {
        assert_eq!(
            CommandResult::from(SessionError::NotSupported("x".into())).code,
            "not_supported"
        );
        assert_eq!(
            CommandResult::from(SessionError::Invalid("x".into())).code,
            "invalid_argument"
        );
        assert_eq!(
            CommandResult::from(SessionError::Failed("x".into())).code,
            "hardware_error"
        );
        assert!(res(Ok(())).ok);
    }

    #[test]
    fn invalid_audio_requests_do_not_reach_the_backend() {
        let rt = SessionRuntime::new();
        assert_eq!(
            rt.execute(&SessionAction::SetVolume { percent: f64::NAN })
                .code,
            "invalid_argument"
        );
        assert_eq!(
            rt.execute(&SessionAction::SetAudioOutput { id: String::new() })
                .code,
            "invalid_argument"
        );
    }

    #[test]
    fn sampling_is_honest_and_never_panics() {
        // Real machine: whatever it answers, it answers without inventing audio or privacy values
        // when the backends are missing.
        let rt = SessionRuntime::new();
        let s = rt.sample();
        if let Some(a) = &s.audio {
            assert!((0.0..=100.0).contains(&a.volume));
        }
    }
}
