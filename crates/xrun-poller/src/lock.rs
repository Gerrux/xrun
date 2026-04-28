#![deny(unsafe_code)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PollerLockError {
    #[error("another poller is already running for this run")]
    AlreadyPolling,
    #[error("I/O error acquiring lock: {0}")]
    Io(#[from] std::io::Error),
}

static ACTIVE_POLLERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_pollers() -> &'static Mutex<HashSet<String>> {
    ACTIVE_POLLERS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Advisory in-process lock for a poller identified by `run_id`.
///
/// Uses a global `HashSet` for in-process exclusion (works across threads in the
/// same process) and writes a PID file for cross-process visibility.
pub struct PollerLock {
    run_id: String,
    pid_file: PathBuf,
}

impl PollerLock {
    /// Try to acquire the lock. Returns `Err(AlreadyPolling)` if another
    /// poller is active for the same `run_id` in this process.
    pub fn try_acquire(run_id: &str, pid_file: PathBuf) -> Result<Self, PollerLockError> {
        let mut guard = active_pollers()
            .lock()
            .expect("poller registry mutex poisoned");
        if guard.contains(run_id) {
            return Err(PollerLockError::AlreadyPolling);
        }
        if let Some(parent) = pid_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&pid_file, std::process::id().to_string())?;
        guard.insert(run_id.to_string());
        Ok(Self {
            run_id: run_id.to_string(),
            pid_file,
        })
    }
}

impl Drop for PollerLock {
    fn drop(&mut self) {
        if let Ok(mut guard) = active_pollers().lock() {
            guard.remove(&self.run_id);
        }
        let _ = std::fs::remove_file(&self.pid_file);
    }
}
