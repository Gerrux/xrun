#![deny(unsafe_code)]

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use xrun_core::{vendor::InstanceHandle, RunId, Store};

use crate::cli::LogsArgs;

pub fn run(args: &LogsArgs, db_path: &Path, runs_dir: &Path) -> Result<()> {
    let id: RunId = args
        .id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.id))?;

    if args.follow {
        return follow_logs(&id, db_path, runs_dir, args.grep.as_deref());
    }

    let log_path = runs_dir.join(id.to_string()).join("stdout.log");

    if !log_path.exists() {
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

/// Dispatch `xrun logs --follow` to either SSH streaming (vast) or local file
/// reading (kaggle, which has no live log streaming).
fn follow_logs(id: &RunId, db_path: &Path, runs_dir: &Path, grep: Option<&str>) -> Result<()> {
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;
    let run = store
        .get_run(id)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {id}"))?;

    if run.vendor == "kaggle" {
        // §1: Kaggle has no live streaming — show the locally pulled log file.
        let log_path = runs_dir.join(id.to_string()).join("stdout.log");
        if log_path.exists() {
            let content = std::fs::read_to_string(&log_path)
                .with_context(|| format!("failed to read log: {}", log_path.display()))?;
            match grep {
                Some(pattern) => {
                    for line in content.lines() {
                        if line.contains(pattern) {
                            println!("{line}");
                        }
                    }
                }
                None => print!("{content}"),
            }
        } else {
            eprintln!(
                "No log available yet for Kaggle run {id}.\n\
                 Live streaming is not supported. Run `xrun pull {id}` after the \
                 kernel completes to download the output."
            );
        }
        return Ok(());
    }

    follow_remote(id, &run, db_path, grep)
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
