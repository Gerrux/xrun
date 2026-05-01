#![deny(unsafe_code)]

//! Subprocess spawning helpers.
//!
//! `run_setup_blocking` runs a script synchronously and surfaces stderr on
//! failure. `spawn_main` starts the long-running training subprocess in the
//! background, redirects stdout+stderr to a file, and returns its PID.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::LocalError;
use crate::shell::ResolvedShell;

/// Best-effort liveness probe for `pid`. Uses `tasklist` on Windows and
/// `kill -0` on Unix so we don't have to introduce libc bindings (would force
/// `unsafe`). Returns `false` on any error so a failed probe doesn't keep a
/// stale instance row "alive".
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // tasklist /FI "PID eq <n>" /FO CSV /NH prints a row when the PID
        // exists, or "INFO: No tasks…" when not. We just check stdout for the
        // PID number.
        let mut cmd = Command::new("tasklist");
        cmd.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        match cmd.output() {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                text.contains(&pid.to_string())
            }
            _ => false,
        }
    }
}

/// Cross-platform process kill. Uses `kill -TERM`/`kill -KILL` on Unix and
/// `taskkill /F /PID` on Windows. Idempotent: a missing PID returns `Ok`.
pub fn kill_process(pid: u32) -> Result<(), LocalError> {
    if !process_alive(pid) {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status();
        // Give it a moment, then SIGKILL if still up.
        std::thread::sleep(std::time::Duration::from_millis(500));
        if process_alive(pid) {
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .status();
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // /F = force, /T = include children. Same model as SIGKILL on a
        // process group.
        let mut cmd = Command::new("taskkill");
        cmd.args(["/F", "/T", "/PID", &pid.to_string()]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        let status = cmd.status().map_err(|e| LocalError::Kill(e.to_string()))?;
        if !status.success() && process_alive(pid) {
            return Err(LocalError::Kill(format!(
                "taskkill exited {} but {pid} still alive",
                status.code().unwrap_or(-1)
            )));
        }
        Ok(())
    }
}

/// Best-effort GPU probe via `nvidia-smi`. Returns one short line per GPU
/// (`"NVIDIA RTX 4090 (12 GB free)"`), or `Ok(empty)` when nvidia-smi is
/// missing or returns a non-zero status. The caller turns this into a
/// `VendorStatus.account` line.
pub fn probe_gpu_summary() -> Vec<String> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args([
        "--query-gpu=name,memory.free",
        "--format=csv,noheader,nounits",
    ]);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|line| {
                    // "NVIDIA GeForce RTX 4090, 23028"
                    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
                    match (parts.first(), parts.get(1)) {
                        (Some(name), Some(mem)) => {
                            format!(
                                "{name} ({} GB free)",
                                mem.parse::<u64>().unwrap_or(0) / 1024
                            )
                        }
                        _ => line.to_string(),
                    }
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run `script` through `shell`, blocking until it exits. Stdout and stderr
/// are inherited (so the launching CLI sees them in real time). On non-zero
/// exit returns `LocalError::SetupFailed` with the captured stderr.
pub fn run_setup_blocking(
    shell: &ResolvedShell,
    script: &str,
    workdir: &Path,
    env: &HashMap<String, String>,
) -> Result<(), LocalError> {
    let mut cmd = Command::new(&shell.binary);
    cmd.args(&shell.leading_args);
    cmd.arg(script);
    cmd.current_dir(workdir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .output()
        .map_err(|e| LocalError::Spawn(format!("setup: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(LocalError::SetupFailed {
            exit_code: output.status.code().unwrap_or(-1),
            stderr,
        });
    }
    Ok(())
}

/// Spawn `script` through `shell`, redirecting stdout+stderr to `stdout_path`
/// (created or appended). Returns the child PID immediately — the parent does
/// not wait. The child becomes orphaned if the parent exits, which is the
/// intended behavior for `--detach` runs.
pub fn spawn_main(
    shell: &ResolvedShell,
    script: &str,
    workdir: &Path,
    env: &HashMap<String, String>,
    stdout_path: &Path,
) -> Result<u32, LocalError> {
    if let Some(parent) = stdout_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stdout_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(stdout_path)?;
    let stderr_file = stdout_file.try_clone()?;

    let mut cmd = Command::new(&shell.binary);
    cmd.args(&shell.leading_args);
    cmd.arg(script);
    cmd.current_dir(workdir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(stdout_file));
    cmd.stderr(Stdio::from(stderr_file));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW only — DETACHED_PROCESS is incompatible with
        // stdio handle inheritance and stdout would land nowhere. The child
        // already outlives its parent on Windows by default.
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let child = cmd
        .spawn()
        .map_err(|e| LocalError::Spawn(format!("main: {e}")))?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::resolve_shell;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn run_setup_succeeds_on_true() {
        let shell = resolve_shell().expect("shell");
        let td = TempDir::new().unwrap();
        let env = HashMap::new();
        // `exit 0` works on bash, sh, pwsh, powershell.
        run_setup_blocking(&shell, "exit 0", td.path(), &env).expect("ok");
    }

    #[test]
    fn run_setup_surfaces_failure() {
        let shell = resolve_shell().expect("shell");
        let td = TempDir::new().unwrap();
        let env = HashMap::new();
        let err = run_setup_blocking(&shell, "exit 7", td.path(), &env)
            .expect_err("must fail with exit 7");
        match err {
            LocalError::SetupFailed { exit_code, .. } => assert_eq!(exit_code, 7),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn spawn_main_writes_stdout_to_file() {
        let shell = resolve_shell().expect("shell");
        let td = TempDir::new().unwrap();
        let stdout = td.path().join("stdout.log");
        let env = HashMap::new();
        // Both `echo hello` and `Write-Host hello` print "hello"; pick the
        // command that the resolved shell will accept.
        let script = match shell.kind {
            crate::shell::ShellKind::Pwsh | crate::shell::ShellKind::PowerShell => {
                "Write-Output hello"
            }
            _ => "echo hello",
        };
        let pid = spawn_main(&shell, script, td.path(), &env, &stdout).expect("spawn");
        assert!(pid > 0);
        // Wait briefly for the child to flush. We poll the file size up to ~3s
        // — enough for any reasonable subprocess startup, well under test
        // timeout. No `sleep`-loop antipattern: this is the only sync wait.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(meta) = std::fs::metadata(&stdout) {
                if meta.len() > 0 {
                    break;
                }
            }
            if std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let content = std::fs::read_to_string(&stdout).expect("read stdout.log");
        assert!(content.trim_end().contains("hello"), "got: {content:?}");
    }
}
