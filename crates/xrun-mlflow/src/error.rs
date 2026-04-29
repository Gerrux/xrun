#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MlflowError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("authentication failed (HTTP 401)")]
    Auth,

    #[error("bad request (HTTP {status}): {body}")]
    BadRequest { status: u16, body: String },

    #[error("not found (HTTP 404): {0}")]
    NotFound(String),

    #[error("conflict (HTTP 409): {0}")]
    Conflict(String),

    #[error("internal server error (HTTP {status}): {body}")]
    Internal { status: u16, body: String },

    #[error("unexpected HTTP {status}: {body}")]
    Unexpected { status: u16, body: String },

    #[error("failed to parse response: {0}")]
    Parse(String),
}
