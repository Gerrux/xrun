#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SshError {
    #[error("ssh host alias not found in credentials.toml: [vendors.ssh.{0}]")]
    HostAliasUnknown(String),

    #[error("ssh host '{alias}' missing required field: {field}")]
    HostFieldMissing { alias: String, field: &'static str },

    #[error("ssh binary not found in PATH: {0}")]
    SshNotFound(String),

    #[error("rsync binary not found in PATH (install rsync or fall back to mode=copy)")]
    RsyncNotFound,

    #[error("ssh exec failed (exit {exit_code}): {stderr}")]
    SshFailure { exit_code: i32, stderr: String },

    #[error("remote file truncated: {file} was {was} bytes, now {now}")]
    Truncated { file: String, was: u64, now: u64 },

    #[error("instance has no recorded PID — cannot stop")]
    NoPid,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<SshError> for xrun_core::error::VendorError {
    fn from(e: SshError) -> Self {
        match e {
            SshError::Io(io) => xrun_core::error::VendorError::Io(io),
            SshError::Truncated { .. } => xrun_core::error::VendorError::Truncated,
            other => xrun_core::error::VendorError::Other(other.to_string()),
        }
    }
}
