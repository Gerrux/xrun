#![deny(unsafe_code)]

//! Core library for xrun: manifest parsing, SQLite storage, config, and domain types.

pub mod config;
pub mod error;
pub mod manifest;
pub mod paths;
pub mod store;

pub use config::{ConfigStore, Credentials, GlobalConfig, InitResult};
pub use error::{ConfigError, ManifestError, StoreError};
pub use manifest::Manifest;
pub use store::{Run, RunId, RunStatus, Store};
