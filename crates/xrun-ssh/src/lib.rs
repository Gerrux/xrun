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

pub use adapter::{
    absolute_shell_path, build_cmd_line, classify_kind, effective_run_dir, pull_pattern,
    remote_run_dir, remote_run_files, remote_run_files_in, resolve_workdir_root, training_dir,
    RemoteRunFiles, SshAdapter,
};
pub use error::SshError;
