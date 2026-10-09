#![deny(unsafe_code)]

use thiserror::Error;
use xrun_core::pybridge::{BridgeError, RemoteKind};

#[derive(Debug, Error)]
pub enum LightningError {
    #[error(
        "Lightning AI authentication failed: {0} (set lightning.api_key + lightning.user_id \
         via `xrun config set`, or run `lightning login`)"
    )]
    Auth(String),

    #[error("Lightning AI: not found: {0}")]
    NotFound(String),

    #[error("Lightning bridge: {0}")]
    Bridge(String),

    #[error("remote command failed (exit {exit_code}): {output}")]
    RemoteExit { exit_code: i32, output: String },

    #[error("remote file truncated")]
    Truncated,

    #[error("unexpected bridge response: {0}")]
    Protocol(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl From<BridgeError> for LightningError {
    fn from(e: BridgeError) -> Self {
        match e {
            BridgeError::Remote {
                kind: RemoteKind::Auth,
                msg,
            } => LightningError::Auth(msg),
            BridgeError::Remote {
                kind: RemoteKind::NotFound,
                msg,
            } => LightningError::NotFound(msg),
            other => LightningError::Bridge(other.to_string()),
        }
    }
}

impl From<LightningError> for xrun_core::error::VendorError {
    fn from(e: LightningError) -> Self {
        match e {
            LightningError::Io(io) => xrun_core::error::VendorError::Io(io),
            LightningError::Truncated => xrun_core::error::VendorError::Truncated,
            other => xrun_core::error::VendorError::Other(other.to_string()),
        }
    }
}
