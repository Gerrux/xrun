#![deny(unsafe_code)]

use std::io::{Read as _, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use xrun_core::{store::RunStatus, vendor::InstanceHandle, GlobalConfig, Run, RunId, Store};

use crate::cli::LogsArgs;

pub fn run(args: &LogsArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let id: RunId = args
        .id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.id))?;

    if args.follow {
        return follow_logs(&id, db_path, runs_dir, config_dir, args.grep.as_deref());
    }

    let log_path = runs_dir.join(id.to_string()).join("stdout.log");

    if !log_path.exists()
        || std::fs::metadata(&log_path)
            .map(|m| m.len() == 0)
            .unwrap_or(true)
    {
        // Empty/missing log on a still-active run is the #1 thing users hit
        // and the silent return makes it feel like xrun is broken. Tell them
        // what we know about *why* there's nothing to show — the answer
        // depends on the vendor and whether MLflow is wired up.
        explain_empty_log(&id, db_path, config_dir);
        return Ok(());
    }

    let content = std::fs::read_to_string(&log_path)
        .with_context(|| format!("failed to read log: {}", log_path.display()))?;

    match &args.grep {
        Some(pattern) => {
            for line in content.lines() {
                if line.contains(pattern.as_str()) {
                    println!("{line}");
                }
            }
        }
        None => print!("{content}"),
    }

    Ok(())
}

/// Diagnose why `stdout.log` is empty for the given run and print a hint to
/// stderr. Best-effort: if anything fails to load we just stay silent.
fn explain_empty_log(id: &RunId, db_path: &Path, config_dir: &Path) {
    let Ok(store) = Store::open(db_path) else {
        return;
    };
    let Ok(Some(run)) = store.get_run(id) else {
        return;
    };
    explain_empty_log_for(id, &run, config_dir);
}

fn explain_empty_log_for(id: &RunId, run: &Run, config_dir: &Path) {
    let status_str = run.status.as_str();
    let active = matches!(status_str, "provisioning" | "uploading" | "running");

    match run.vendor.as_str() {
        "kaggle" => {
            let mlflow_configured = GlobalConfig::load(config_dir)
                .map(|g| g.mlflow.url.is_some())
                .unwrap_or(false);
            if !mlflow_configured {
                eprintln!(
                    "No live log available for Kaggle run {id}. Kaggle's API has no \
                     log-streaming endpoint, so xrun pipes logs through MLflow when \
                     it's configured. Set `mlflow.url` in `~/.config/xrun/config.toml` \
                     to enable live tailing, or wait for the kernel to finish and \
                     run `xrun pull {id}` to download the output."
                );
            } else if active {
                eprintln!(
                    "No log chunks have arrived yet for Kaggle run {id}. The kernel \
                     is still warming up — xrun_hook needs a few seconds after \
                     `running:start` before the first chunk lands in MLflow. Try \
                     again in ~10s."
                );
            } else {
                eprintln!(
                    "No log captured for Kaggle run {id} (status={status_str}). Run \
                     `xrun pull {id}` to fetch the kernel output if it ever finished."
                );
            }
        }
        _ if active => {
            eprintln!(
                "No log captured yet for run {id} (status={status_str}). The poll-daemon \
                 snapshots stdout every few seconds — try again shortly, or use \
                 `xrun logs {id} --follow` to stream live."
            );
        }
        _ => {
            eprintln!("No log captured for run {id} (status={status_str}).");
        }
    }
}

/// Dispatch `xrun logs --follow` to either SSH streaming (vast) or local-file
/// tailing (kaggle / local — the poll-daemon snapshots stdout into the local
/// run directory; we just keep reading from where it grows).
fn follow_logs(
    id: &RunId,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
    grep: Option<&str>,
) -> Result<()> {
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;
    let run = store
        .get_run(id)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {id}"))?;

    if run.vendor == "kaggle" || run.vendor == "local" {
        return follow_local_file(id, &run, db_path, runs_dir, config_dir, grep);
    }

    follow_remote(id, &run, db_path, grep)
}

/// Tail `runs_dir/<id>/stdout.log`, sleeping briefly between polls until the
/// run reaches a terminal status. The poll-daemon is the producer (it pulls
/// MLflow log chunks for Kaggle, or reads the local stdout file for local
/// runs); this function is the consumer that streams what lands locally.
fn follow_local_file(
    id: &RunId,
    run: &Run,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
    grep: Option<&str>,
) -> Result<()> {
    let log_path = runs_dir.join(id.to_string()).join("stdout.log");
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let mut offset: u64 = 0;
    let mut warned_empty = false;
    let mut buf = String::new();

    loop {
        // Re-read terminal status each iteration so we exit promptly.
        let current = store.get_run(id)?;
        let status = current
            .as_ref()
            .map(|r| r.status.clone())
            .unwrap_or_else(|| run.status.clone());
        let terminal = matches!(
            status,
            RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled
        );

        match std::fs::File::open(&log_path) {
            Ok(mut f) => {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                if len < offset {
                    // File was truncated (e.g. pre-emption + restart on vast,
                    // or rerun); start from the new beginning.
                    offset = 0;
                }
                if len > offset {
                    f.seek(SeekFrom::Start(offset))?;
                    buf.clear();
                    f.read_to_string(&mut buf).ok();
                    offset = len;
                    match grep {
                        Some(pattern) => {
                            for line in buf.lines() {
                                if line.contains(pattern) {
                                    println!("{line}");
                                }
                            }
                        }
                        None => print!("{buf}"),
                    }
                } else if !warned_empty && len == 0 {
                    explain_empty_log_for(id, run, config_dir);
                    warned_empty = true;
                }
            }
            Err(_) => {
                if !warned_empty {
                    explain_empty_log_for(id, run, config_dir);
                    warned_empty = true;
                }
            }
        }

        if terminal {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// `xrun logs -f <id>` streams the live stdout from the running instance over
/// ssh. Doesn't go through the poller — the poller already snapshots the tail
/// every few seconds, but for an active debug session a 0-latency stream is
/// what you actually want.
fn follow_remote(
    id: &RunId,
    run: &xrun_core::Run,
    db_path: &Path,
    grep: Option<&str>,
) -> Result<()> {
    let instance_id = run
        .instance_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("run {id} has no instance yet"))?;
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;
    let inst = store
        .get_instance(instance_id)?
        .ok_or_else(|| anyhow::anyhow!("instance {instance_id} not found"))?;
    let state_json = inst
        .state_json
        .ok_or_else(|| anyhow::anyhow!("instance {instance_id} has no stored handle"))?;
    let handle: InstanceHandle =
        serde_json::from_str(&state_json).context("failed to deserialize instance handle")?;

    let host = handle
        .ssh_host
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("instance has no ssh_host"))?;
    let port = handle
        .ssh_port
        .ok_or_else(|| anyhow::anyhow!("instance has no ssh_port"))?;
    let user = if handle.ssh_user.is_empty() {
        "root"
    } else {
        handle.ssh_user.as_str()
    };

    // tail -F (capital F) keeps following across log rotations and waits for
    // the file to exist if the run hasn't started writing yet. Pipe through
    // grep --line-buffered so a filter pattern doesn't bottleneck on the
    // local pipe.
    let remote_cmd = match grep {
        Some(p) => format!(
            "tail -n 200 -F /workspace/run/stdout.log 2>/dev/null | grep --line-buffered -- {}",
            shell_escape(p)
        ),
        None => "tail -n 200 -F /workspace/run/stdout.log 2>/dev/null".to_string(),
    };

    let status = Command::new("ssh")
        .arg("-p")
        .arg(port.to_string())
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg(format!("{user}@{host}"))
        .arg(remote_cmd)
        .status()
        .with_context(|| "failed to spawn ssh")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Single-quote-escape a string for safe inclusion inside a remote shell
/// `grep -- '<pattern>'` argument. Any embedded single quotes are split out.
fn shell_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}
