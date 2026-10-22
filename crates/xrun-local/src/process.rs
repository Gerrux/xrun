#![deny(unsafe_code)]

//! Subprocess spawning helpers.
//!
//! `run_setup_blocking` runs a script synchronously and surfaces stderr on
//! failure. `spawn_main` starts the long-running training subprocess in the
//! background, redirects stdout+stderr to a file, and returns its PID.

use std::collections::HashMap;
use std::fs::OpenOptions;

use chrono::{DateTime, Utc};
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
        let exists = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !exists {
            return false;
        }
        // `kill -0` succeeds for zombie PIDs — the kernel keeps the entry
        // until the parent reaps. spawn_main returns the PID and drops the
        // `Child`, so nothing ever wait()s our subprocess; without this
        // check, a SIGKILL'd child stays "alive" until the test process exits.
        #[cfg(target_os = "linux")]
        {
            if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
                for line in status.lines() {
                    if let Some(rest) = line.strip_prefix("State:") {
                        let st = rest.trim();
                        if st.starts_with('Z') || st.starts_with('X') {
                            return false;
                        }
                    }
                }
            }
        }
        // macOS has no /proc — fall back to `ps -o stat=` which prints the
        // single-letter state column ('Z' for zombie).
        #[cfg(target_os = "macos")]
        {
            if let Ok(out) = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
            {
                if out.status.success() {
                    let st = String::from_utf8_lossy(&out.stdout);
                    if st.trim().starts_with('Z') {
                        return false;
                    }
                }
            }
        }
        true
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

/// Clock slack applied by [`process_alive_since`] callers: the training PID is
/// recorded right after `spawn_main`, so a genuine training process started
/// at or before that moment; the slack absorbs coarse start-time clocks
/// (`/proc/stat` `btime` has 1 s granularity, Windows `StartTime` is rounded)
/// without letting a PID recycled long after launch pass as the original.
pub const START_SLACK_SECS: i64 = 5;

/// Wall-clock start time of the process `pid`, or `None` when it cannot be
/// determined (process gone, access denied, unsupported platform).
///
/// - Linux: `/proc/<pid>/stat` field 22 (`starttime`, clock ticks since boot)
///   plus `btime` from `/proc/stat`. `CLK_TCK` is assumed to be 100 — the
///   value on every mainstream Linux build — since reading it needs libc.
/// - Windows: one `powershell` call (`Get-Process … .StartTime`).
/// - macOS: `ps -o lstart=`, which prints local time.
pub fn process_started_at(pid: u32) -> Option<DateTime<Utc>> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let btime = std::fs::read_to_string("/proc/stat").ok()?;
        linux_started_at(&stat, &btime)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToUniversalTime().ToString('o')"
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        cmd.stdin(Stdio::null());
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|t| t.with_timezone(&Utc))
    }
    #[cfg(target_os = "macos")]
    {
        use chrono::{Local, NaiveDateTime, TimeZone};
        let out = Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        // "Thu Oct 22 10:00:00 2026"; the day is space-padded, so squeeze
        // whitespace before parsing.
        let squeezed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let naive = NaiveDateTime::parse_from_str(&squeezed, "%a %b %e %H:%M:%S %Y").ok()?;
        Local
            .from_local_datetime(&naive)
            .single()
            .map(|t| t.with_timezone(&Utc))
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Opaque identity of the process `pid` that does not move with the wall
/// clock, or `None` when it cannot be read. Two reads for the same process
/// compare equal; a different process holding a recycled PID gives a
/// different token.
///
/// A start time derived from the clock is not enough: on Linux it is
/// `btime + ticks`, and `btime` shifts with every realtime step (WSL2 after
/// host sleep, an NTP step), so a live process could look "started after
/// run.pid was written". The token is the raw value the kernel stored at
/// creation instead.
///
/// - Linux: `starttime` from `/proc/<pid>/stat` (clock ticks since boot).
/// - Windows: `StartTime` as a UTC FILETIME (one `powershell` call).
/// - macOS: `ps -o lstart=` verbatim.
pub fn process_start_token(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        linux_start_ticks(&stat).map(|t| t.to_string())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!("(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToFileTimeUtc()");
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        cmd.stdin(Stdio::null());
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        text.parse::<u64>().ok().map(|t| t.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let squeezed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        (!squeezed.is_empty()).then_some(squeezed)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// `starttime` (field 22) of a `/proc/<pid>/stat` line.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_start_ticks(stat: &str) -> Option<i64> {
    // `comm` (field 2) is parenthesised and may itself contain spaces and
    // parentheses, so anchor on the last ')'. After it, fields start at #3
    // (state), making `starttime` (#22) the 20th token.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// Pure part of the Linux start-time lookup, split out so it is testable on
/// any platform. `stat` is `/proc/<pid>/stat`, `proc_stat` is `/proc/stat`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_started_at(stat: &str, proc_stat: &str) -> Option<DateTime<Utc>> {
    const CLK_TCK: i64 = 100;
    let ticks = linux_start_ticks(stat)?;
    let btime: i64 = proc_stat
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    let secs = btime + ticks / CLK_TCK;
    let nanos = ((ticks % CLK_TCK) * (1_000_000_000 / CLK_TCK)) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// Liveness probe that survives PID reuse: `pid` counts as alive only if the
/// process exists AND it was not started after `not_after`.
///
/// After a host restart the PID recorded in `run.pid` may belong to an
/// unrelated process; the real training process would have started before
/// `not_after` (the moment the PID was recorded). When the start time cannot
/// be read the check degrades to plain [`process_alive`].
pub fn process_alive_since(pid: u32, not_after: DateTime<Utc>) -> bool {
    if !process_alive(pid) {
        return false;
    }
    match process_started_at(pid) {
        Some(started) => started <= not_after,
        None => true,
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
    fn linux_started_at_parses_stat_with_tricky_comm() {
        // comm contains spaces and a ')'; starttime = 123456 ticks = 1234.56 s.
        let stat = "42 (we ird) name) S 1 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 123456 1000 10 18446744073709551615";
        let proc_stat = "cpu  1 2 3\nbtime 1700000000\nprocesses 5\n";
        let t = linux_started_at(stat, proc_stat).expect("parsed");
        assert_eq!(t.timestamp(), 1_700_000_000 + 1234);
        assert_eq!(t.timestamp_subsec_millis(), 560);
        assert!(linux_started_at("garbage", proc_stat).is_none());
        assert!(linux_started_at(stat, "cpu 1\n").is_none());
    }

    #[test]
    fn current_process_start_time_is_known_and_in_the_past() {
        let started = process_started_at(std::process::id());
        // Platforms where the lookup is unsupported return None; everywhere
        // else it must be a plausible instant.
        if let Some(t) = started {
            assert!(t <= Utc::now() + chrono::Duration::seconds(START_SLACK_SECS));
            assert!(t > Utc::now() - chrono::Duration::days(365));
        } else if cfg!(any(target_os = "linux", windows)) {
            panic!("start time must be readable on linux/windows");
        }
    }

    #[test]
    fn alive_since_rejects_a_process_started_after_the_cutoff() {
        let pid = std::process::id();
        let now = Utc::now();
        // Cutoff in the future: the live test process started before it.
        assert!(process_alive_since(pid, now + chrono::Duration::hours(1)));
        // Cutoff far in the past (a PID recorded before a reboot): the
        // current process "started after it", i.e. the PID was recycled.
        // Only decidable where the start time is readable.
        if process_started_at(pid).is_some() {
            assert!(!process_alive_since(
                pid,
                now - chrono::Duration::days(3650)
            ));
        }
    }

    #[test]
    fn alive_since_false_for_dead_pid() {
        // u32::MAX is never a valid PID on any supported platform.
        assert!(!process_alive_since(
            u32::MAX - 1,
            Utc::now() + chrono::Duration::hours(1)
        ));
    }

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
