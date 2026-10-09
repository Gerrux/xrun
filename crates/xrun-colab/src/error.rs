#![deny(unsafe_code)]

use thiserror::Error;
use xrun_core::pybridge::{BridgeError, RemoteKind};

#[derive(Debug, Error)]
pub enum ColabError {
    /// Transport / protocol failure of the Python bridge, or an error the
    /// Python side reported.
    #[error("{0}")]
    Bridge(#[from] BridgeError),

    /// A snippet ran in the kernel but its output could not be understood.
    #[error("unexpected kernel output for {what}: {detail}")]
    BadOutput { what: &'static str, detail: String },

    /// A remote shell command exited non-zero.
    #[error("remote command failed (exit {code}): {detail}")]
    RemoteCommand { code: i64, detail: String },

    #[error("remote file truncated: {file} was {was} bytes, now {now}")]
    Truncated { file: String, was: u64, now: u64 },

    #[error("not logged in to Google Colab: run `xrun config login colab` in a terminal")]
    NotLoggedIn,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl ColabError {
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            ColabError::NotLoggedIn
                | ColabError::Bridge(BridgeError::Remote {
                    kind: RemoteKind::Auth,
                    ..
                })
        )
    }
}

impl From<ColabError> for xrun_core::error::VendorError {
    fn from(e: ColabError) -> Self {
        match e {
            ColabError::Io(io) => xrun_core::error::VendorError::Io(io),
            ColabError::Truncated { .. } => xrun_core::error::VendorError::Truncated,
            other => xrun_core::error::VendorError::Other(other.to_string()),
        }
    }
}
