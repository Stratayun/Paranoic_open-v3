use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, CoreError>;

/// Stable machine-readable errors; hosts localize these instead of parsing text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    InvalidPath,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    Unsupported,
    LimitExceeded,
    Busy,
    Cancelled,
    SourceChanged,
    Integrity,
    Io,
    Journal,
    NeedsAttention,
    Shutdown,
    Internal,
}

/// Messages intentionally exclude absolute paths, file contents and OS details.
#[derive(Clone, Debug, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn io(error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        let code = match error.kind() {
            ErrorKind::NotFound => ErrorCode::NotFound,
            ErrorKind::AlreadyExists => ErrorCode::AlreadyExists,
            ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            ErrorKind::Unsupported => ErrorCode::Unsupported,
            _ => ErrorCode::Io,
        };
        Self::new(code, "storage operation failed")
    }

    pub(crate) fn journal(_: rusqlite::Error) -> Self {
        Self::new(ErrorCode::Journal, "operation journal is unavailable or invalid")
    }

    pub(crate) fn internal() -> Self {
        Self::new(ErrorCode::Internal, "core synchronization failed")
    }
}
