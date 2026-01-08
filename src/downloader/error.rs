//! Error types for the downloader module

use std::fmt;

/// Error type for download operations
#[derive(Debug, Clone)]
pub struct DownloadError {
    pub kind: ErrorKind,
    pub message: String,
}

/// Kinds of download errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// Network error (connection failed, timeout, etc.)
    Network,
    /// File system error (permission denied, disk full, etc.)
    FileSystem,
    /// Invalid input (bad URL, invalid path, etc.)
    InvalidInput,
    /// Task not found
    TaskNotFound,
    /// Task already exists
    TaskAlreadyExists,
    /// Download was cancelled
    Cancelled,
    /// Operation not supported by this downloader implementation
    Unsupported,
    /// Unknown error
    Unknown,
}

impl DownloadError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    pub fn file_system(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::FileSystem, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn task_not_found(task_id: impl fmt::Display) -> Self {
        Self::new(
            ErrorKind::TaskNotFound,
            format!("Task not found: {}", task_id),
        )
    }

    pub fn task_already_exists(task_id: impl fmt::Display) -> Self {
        Self::new(
            ErrorKind::TaskAlreadyExists,
            format!("Task already exists: {}", task_id),
        )
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Cancelled, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unknown, message)
    }
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for DownloadError {}

impl From<std::io::Error> for DownloadError {
    fn from(err: std::io::Error) -> Self {
        Self::file_system(err.to_string())
    }
}

/// Result type for download operations
pub type Result<T> = std::result::Result<T, DownloadError>;
