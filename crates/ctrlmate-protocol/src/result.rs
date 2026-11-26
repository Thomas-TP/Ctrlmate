//! Command results: stable machine codes, never human sentences (architecture contract section 4).

use serde::{Deserialize, Serialize};

/// Stable, lower-snake result codes. The UI and the Home Assistant integration translate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultCode {
    Ok,
    PermissionDenied,
    NoAgent,
    AgentTimeout,
    NotSupported,
    InvalidArgument,
    HardwareError,
    Busy,
    Unavailable,
    Internal,
}

impl ResultCode {
    /// Wire representation of the code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::PermissionDenied => "permission_denied",
            Self::NoAgent => "no_agent",
            Self::AgentTimeout => "agent_timeout",
            Self::NotSupported => "not_supported",
            Self::InvalidArgument => "invalid_argument",
            Self::HardwareError => "hardware_error",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }

    /// Parses a wire code; unknown codes map to `None`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ok" => Self::Ok,
            "permission_denied" => Self::PermissionDenied,
            "no_agent" => Self::NoAgent,
            "agent_timeout" => Self::AgentTimeout,
            "not_supported" => Self::NotSupported,
            "invalid_argument" => Self::InvalidArgument,
            "hardware_error" => Self::HardwareError,
            "busy" => Self::Busy,
            "unavailable" => Self::Unavailable,
            "internal" => Self::Internal,
            _ => return None,
        })
    }
}

/// Outcome of a command. `ok` and `message` are kept for older Home Assistant builds;
/// `code` is the stable discriminator. `message` is technical detail only (never UI text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
    pub ok: bool,
    /// One of [`ResultCode::as_str`]; `"ok"` iff `ok == true`.
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl CommandResult {
    pub fn ok() -> Self {
        Self {
            ok: true,
            code: ResultCode::Ok.as_str().into(),
            message: None,
        }
    }

    /// Success carrying technical detail (e.g. captured script output).
    pub fn ok_with(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            code: ResultCode::Ok.as_str().into(),
            message: Some(message.into()),
        }
    }

    /// Failure without detail. `code` must not be [`ResultCode::Ok`].
    pub fn err(code: ResultCode) -> Self {
        debug_assert!(code != ResultCode::Ok);
        Self {
            ok: false,
            code: code.as_str().into(),
            message: None,
        }
    }

    pub fn err_with(code: ResultCode, message: impl Into<String>) -> Self {
        debug_assert!(code != ResultCode::Ok);
        Self {
            ok: false,
            code: code.as_str().into(),
            message: Some(message.into()),
        }
    }

    /// Parsed code; unknown codes (from a newer peer) read as `Internal`.
    pub fn result_code(&self) -> ResultCode {
        ResultCode::parse(&self.code).unwrap_or(ResultCode::Internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_roundtrip() {
        for c in [
            ResultCode::Ok,
            ResultCode::PermissionDenied,
            ResultCode::NoAgent,
            ResultCode::AgentTimeout,
            ResultCode::NotSupported,
            ResultCode::InvalidArgument,
            ResultCode::HardwareError,
            ResultCode::Busy,
            ResultCode::Unavailable,
            ResultCode::Internal,
        ] {
            assert_eq!(ResultCode::parse(c.as_str()), Some(c));
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.as_str()));
        }
        assert_eq!(ResultCode::parse("nope"), None);
    }

    #[test]
    fn result_wire_shape() {
        let ok = serde_json::to_value(CommandResult::ok()).unwrap();
        assert_eq!(ok, serde_json::json!({"ok": true, "code": "ok"}));
        let err = serde_json::to_value(CommandResult::err_with(ResultCode::NoAgent, "x")).unwrap();
        assert_eq!(
            err,
            serde_json::json!({"ok": false, "code": "no_agent", "message": "x"})
        );
        assert!(serde_json::from_str::<CommandResult>(r#"{"ok":true}"#).is_err());
    }
}
