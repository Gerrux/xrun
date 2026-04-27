#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("Validation error: {0}")]
    Validation(String),
}
