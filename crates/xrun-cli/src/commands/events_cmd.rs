#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{RunId, Store};

use crate::cli::EventsArgs;

pub fn run(args: &EventsArgs, db_path: &Path) -> Result<()> {
    if args.follow {
        anyhow::bail!("--follow is not supported in v0.1");
    }

    let id: RunId = args
        .id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.id))?;

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let run = store
        .get_run(&id)
        .context("failed to query run")?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", args.id))?;

    let events = store
        .list_events(&run.id)
        .context("failed to list events")?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string())
        );
    } else if events.is_empty() {
        println!("no events for run {}", args.id);
    } else {
        println!("{:<24}  {:<20}  {:<8}  msg", "ts", "stage", "status");
        println!("{}", "-".repeat(70));
        for e in &events {
            println!(
                "{:<24}  {:<20}  {:<8}  {}",
                e.ts.format("%Y-%m-%dT%H:%M:%SZ"),
                e.stage,
                e.status,
                e.msg.as_deref().unwrap_or("")
            );
        }
    }

    Ok(())
}
