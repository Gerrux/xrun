#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::RunId;

use crate::cli::LogsArgs;

pub fn run(args: &LogsArgs, runs_dir: &Path) -> Result<()> {
    if args.follow {
        anyhow::bail!("--follow is not supported in v0.1");
    }

    let id: RunId = args
        .id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.id))?;

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
