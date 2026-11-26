//! `ctrlmate-agent`: the headless per-user process of Ctrlmate (architecture contract sections 1-3).
//!
//! It runs in the interactive session and does everything the daemon cannot do from session 0 or a
//! system service: lock, displays off, notifications, volume / mute / output, foreground app and
//! gaming detection, microphone / webcam, and custom scripts. It connects to the daemon's loopback
//! `/ipc/agent` WebSocket, authenticates with the IPC token (first frame), streams its
//! `SessionState` (on change and at least every 5 s) and executes the daemon's requests through
//! [`ctrlmate_hal::session`].

pub mod autostart;
pub mod client;
pub mod instance;
pub mod logging;
pub mod token;

pub use client::{ClientConfig, SessionApi, run};

/// Agent version reported in `hello`.
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");
