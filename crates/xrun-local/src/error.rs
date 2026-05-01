#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LocalError {
    #[error("no shell found on PATH (looked for: {0})")]
    NoShell(String),

    #[error("process spawn failed: {0}")]
    Spawn(String),

    #[error("setup step failed (exit {exit_code}): {stderr}")]
    SetupFailed { exit_code: i32, stderr: String },

    #[error("file not found: {0}")]
    FileNotFound(String),

    #[error("instance has no recorded PID — cannot stop")]
    NoPid,

    #[error("kill failed: {0}")]
    Kill(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<LocalError> for xrun_core::error::VendorError {
    fn from(e: LocalError) -> Self {
        match e {
            LocalError::Io(io) => xrun_core::error::VendorError::Io(io),
            other => xrun_core::error::VendorError::Other(other.to_string()),
        }
    }
}
