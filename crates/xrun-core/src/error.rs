#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("Validation error: {0}")]
    Validation(String),
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("schema version {found} is newer than supported {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("run not found: {0}")]
    RunNotFound(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum JsonlError {
    #[error("JSON parse error: {0}")]
    Json(serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum VendorError {
    #[error("not implemented")]
    NotImplemented,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),
    #[error("remote file was truncated (pre-emption restart?)")]
    Truncated,
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("TOML serialize error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("cannot determine project paths: {0}")]
    NoPaths(String),
}
