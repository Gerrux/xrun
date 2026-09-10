#![deny(unsafe_code)]

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
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

/// Advisory process and thread lock for a poller identified by `run_id`.
///
/// Uses a global `HashSet` for in-process exclusion (works across threads in the
/// same process), an OS file lock across processes, and a readable PID file.
pub struct PollerLock {
    run_id: String,
    pid_file: PathBuf,
    _lock_file: File,
}

impl PollerLock {
    /// Try to acquire the lock. Returns `Err(AlreadyPolling)` if another
    /// poller is active for the same run. The OS releases the lock on exit.
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
        // Never unlink this file: waiters must keep locking the same inode.
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(pid_file.with_extension("lock"))?;
        if let Err(e) = fs2::FileExt::try_lock_exclusive(&lock_file) {
            if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                return Err(PollerLockError::AlreadyPolling);
            }
            return Err(PollerLockError::Io(e));
        }
        std::fs::write(&pid_file, std::process::id().to_string())?;
        guard.insert(run_id.to_string());
        Ok(Self {
            run_id: run_id.to_string(),
            pid_file,
            _lock_file: lock_file,
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
