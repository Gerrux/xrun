#![deny(unsafe_code)]

//! Local vendor adapter for xrun — runs experiments as host subprocesses
//! without SSH or network. Useful for debugging manifests before paying for
//! cloud time.

pub mod adapter;
pub mod error;
pub mod process;
pub mod shell;
pub mod tail;
pub mod transfer;

pub use adapter::LocalAdapter;
pub use error::LocalError;
