#![deny(unsafe_code)]

//! Core library for xrun: manifest parsing, SQLite storage, config, and domain types.

pub mod error;
pub mod manifest;
pub mod store;

pub use error::{ManifestError, StoreError};
pub use manifest::Manifest;
pub use store::{Run, RunId, RunStatus, Store};
