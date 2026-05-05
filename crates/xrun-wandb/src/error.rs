#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WandbError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    /// HTTP 401 from WandB API. Almost always means a stale or revoked
    /// `wandb_v1_*` key.
    #[error("authentication failed (HTTP 401)")]
    Auth,

    /// GraphQL returned a structured error in the `errors[]` array.
    /// WandB uses this for non-HTTP business-logic failures (project
    /// permission denied, run already exists, schema violations).
    #[error("graphql error: {0}")]
    GraphQl(String),

    #[error("bad request (HTTP {status}): {body}")]
    BadRequest { status: u16, body: String },

    #[error("not found (HTTP 404): {0}")]
    NotFound(String),

    #[error("server error (HTTP {status}): {body}")]
    Server { status: u16, body: String },

    #[error("unexpected HTTP {status}: {body}")]
    Unexpected { status: u16, body: String },

    #[error("failed to parse response: {0}")]
    Parse(String),

    #[error("misconfigured: {0}")]
    Config(String),
}
