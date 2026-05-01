#![deny(unsafe_code)]

//! Thin process-spawn wrappers around `ssh` and `rsync`. Side-effecting
//! counterparts to the pure builders in `cmd.rs`.

use std::process::{Command, Stdio};

use crate::cmd::{self, SshConn};
use crate::error::SshError;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run `ssh ... -- <remote_cmd>` and return captured stdout. Non-zero exit
/// surfaces as `SshError::SshFailure` with the remote stderr attached.
pub fn ssh_exec(conn: &SshConn, remote_cmd: &str) -> Result<Vec<u8>, SshError> {
    let argv = cmd::ssh_argv(conn, remote_cmd);
    let mut cmd = Command::new("ssh");
    cmd.args(&argv);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SshError::SshNotFound("ssh".to_string())
        } else {
            SshError::Io(e)
        }
    })?;
    if !output.status.success() {
        return Err(SshError::SshFailure {
            exit_code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output.stdout)
}

/// Run `rsync …` (any direction). Returns Ok on success, error on non-zero.
pub fn rsync(argv: &[String]) -> Result<(), SshError> {
    let mut cmd = Command::new("rsync");
    cmd.args(argv);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SshError::RsyncNotFound
        } else {
            SshError::Io(e)
        }
    })?;
    if !output.status.success() {
        return Err(SshError::SshFailure {
            exit_code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Probe `wc -c < <file>` over ssh and parse the integer reply.
pub fn remote_file_size(conn: &SshConn, file: &str) -> Result<u64, SshError> {
    let bytes = ssh_exec(conn, &cmd::remote_size_script(file))?;
    let s = String::from_utf8_lossy(&bytes);
    s.trim().parse::<u64>().map_err(|_| SshError::SshFailure {
        exit_code: 0,
        stderr: format!("wc -c returned non-integer: {s:?}"),
    })
}

/// `tail -c +N <file>` over ssh — incremental reader.
pub fn remote_tail(conn: &SshConn, file: &str, offset: u64) -> Result<Vec<u8>, SshError> {
    ssh_exec(conn, &cmd::remote_tail_script(file, offset))
}
