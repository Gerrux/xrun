#![deny(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use xrun_core::{RunId, RunStatus, Store, StoredEvent};

use crate::cli::EventsArgs;

pub fn run(args: &EventsArgs, db_path: &Path) -> Result<()> {
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

    if args.json && !args.follow {
        println!(
            "{}",
            serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string())
        );
        return Ok(());
    }

    if !args.follow {
        if events.is_empty() {
            println!("no events for run {}", args.id);
        } else {
            print_header();
            for e in &events {
                print_event(e);
            }
        }
        return Ok(());
    }

    // --- follow mode ---
    print_header();
    let mut last_id = 0i64;
    for e in &events {
        print_event(e);
        last_id = last_id.max(e.id);
    }

    if is_terminal(&run.status) {
        return Ok(());
    }

    loop {
        std::thread::sleep(Duration::from_secs(1));

        let new_events = store
            .list_events_after(&id, last_id)
            .context("failed to poll events")?;
        for e in &new_events {
            print_event(e);
            last_id = last_id.max(e.id);
        }

        let current = store
            .get_run(&id)
            .context("failed to re-query run")?
            .ok_or_else(|| anyhow::anyhow!("run disappeared"))?;
        if is_terminal(&current.status) {
            // Flush any events that arrived in the same tick as the terminal status.
            let final_events = store
                .list_events_after(&id, last_id)
                .context("failed to flush final events")?;
            for e in &final_events {
                print_event(e);
            }
            eprintln!("run {} {}", args.id, current.status.as_str());
            break;
        }
    }

    Ok(())
}

fn is_terminal(status: &RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled
    )
}

fn print_header() {
    println!("{:<24}  {:<20}  {:<8}  msg", "ts", "stage", "status");
    println!("{}", "-".repeat(70));
}

fn print_event(e: &StoredEvent) {
    println!(
        "{:<24}  {:<20}  {:<8}  {}",
        e.ts.format("%Y-%m-%dT%H:%M:%SZ"),
        e.stage,
        e.status,
        e.msg.as_deref().unwrap_or("")
    );
}
