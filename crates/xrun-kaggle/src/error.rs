#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum KaggleError {
    #[error("kaggle binary not found in PATH: {0}")]
    NotFound(String),

    #[error("kaggle CLI failed (exit {exit_code}): {stderr}")]
    CliFailure { exit_code: i32, stderr: String },

    #[error("failed to parse kaggle output: {0}")]
    ParseError(String),

    #[error("kernel in error state: {0}")]
    KernelError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("store error: {0}")]
    Store(#[from] xrun_core::StoreError),
}
