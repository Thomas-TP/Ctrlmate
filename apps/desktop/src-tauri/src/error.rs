//! Typed error codes returned to the webview. Rust never supplies sentences: the UI translates
//! `code`; `detail` is optional technical text for diagnostics only.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The requested method/path is not on the allowlist.
    RouteNotAllowed,
    /// The request could not be built (bad body, bad id).
    BadRequest,
    /// No IPC token file/env var found: daemon never started or not installed.
    TokenNotFound,
    /// A token file exists but cannot be read (e.g. Linux user not in group `ctrlmate`).
    TokenPermissionDenied,
    /// A token source exists but is malformed.
    TokenInvalid,
    /// The daemon rejected every token we know (401).
    Unauthorized,
    /// Nothing listens on the IPC port, or the connection broke.
    DaemonOffline,
    /// The daemon answered with something we cannot use.
    DaemonProtocol,
    /// The platform does not support this operation.
    NotSupported,
    /// A helper binary (`ctrlmated`, `ctrlmate-agent`) was not found next to the app.
    BinaryNotFound,
    /// Nothing to do (e.g. no update was found by the last check).
    NothingToDo,
    Internal,
}

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{code:?}")]
pub struct IpcError {
    pub code: ErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl IpcError {
    pub fn new(code: ErrorCode) -> Self {
        Self { code, detail: None }
    }

    pub fn with_detail(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: Some(detail.into()),
        }
    }
}

impl From<ErrorCode> for IpcError {
    fn from(code: ErrorCode) -> Self {
        Self::new(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_snake_case_code_only() {
        let v = serde_json::to_value(IpcError::new(ErrorCode::TokenPermissionDenied))
            .unwrap_or_default();
        assert_eq!(v, serde_json::json!({"code": "token_permission_denied"}));
        let v = serde_json::to_value(IpcError::with_detail(ErrorCode::Internal, "x"))
            .unwrap_or_default();
        assert_eq!(v["detail"], "x");
    }
}
