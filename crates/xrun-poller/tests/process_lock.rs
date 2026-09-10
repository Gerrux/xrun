use std::process::{Command, Stdio};
use xrun_poller::lock::{PollerLock, PollerLockError};

#[test]
fn lock_child() {
    let Ok(path) = std::env::var("XRUN_TEST_LOCK_PATH") else {
        return;
    };
    let result = PollerLock::try_acquire("shared-run", path.into());
    if std::env::var_os("XRUN_TEST_LOCK_CONTENDED").is_some() {
        assert!(matches!(result, Err(PollerLockError::AlreadyPolling)));
    } else {
        assert!(result.is_ok());
    }
}

#[test]
fn lock_excludes_other_process_and_releases_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let pid_file = tmp.path().join("poller.pid");
    let lock = PollerLock::try_acquire("shared-run", pid_file.clone()).unwrap();
    let child = |contended: bool| {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "lock_child", "--nocapture"])
            .env("XRUN_TEST_LOCK_PATH", &pid_file)
            .env_remove("XRUN_TEST_LOCK_CONTENDED")
            .stdin(Stdio::null());
        if contended {
            cmd.env("XRUN_TEST_LOCK_CONTENDED", "1");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        assert!(cmd.status().unwrap().success());
    };
    child(true);
    drop(lock);
    child(false);
}
