#![deny(unsafe_code)]

//! Generic SSH vendor adapter for xrun. Targets a host configured in
//! `~/.config/xrun/credentials.toml` under `[vendors.ssh.<alias>]`. The
//! machine is assumed to be always on, so `provision()` and `destroy()`
//! don't allocate or free hardware — they just create/clear a per-run
//! workdir on the remote.

pub mod adapter;
pub mod cmd;
pub mod error;
pub mod ssh;

pub use adapter::SshAdapter;
pub use error::SshError;
