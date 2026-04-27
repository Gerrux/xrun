#![deny(unsafe_code)]

//! Core library for xrun: manifest parsing, SQLite storage, config, and domain types.

pub mod error;
pub mod manifest;

pub use error::ManifestError;
pub use manifest::Manifest;
