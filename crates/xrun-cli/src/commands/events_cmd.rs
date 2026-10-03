#![deny(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use xrun_core::{RunStatus, StoredEvent};

use crate::cli::EventsArgs;
use crate::commands::common::{open_store, resolve_run};

pub fn run(args: &EventsArgs, db_path: &Path) -> Result<()> {
    let store = open_store(db_path)?;
    let run = resolve_run(&store, &args.id)?;
    let id = run.id.clone();

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
            println!("no events for run {}", run.id);
        } else {
            print_header();
            for e in &events {
                print_event(e);
            }
        }
        return Ok(());
    }

    // --- follow mode ---
    if !args.json {
        print_header();
    }
    let mut last_id = 0i64;
    for e in &events {
        emit_event(e, args.json)?;
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
            emit_event(e, args.json)?;
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
                emit_event(e, args.json)?;
            }
            eprintln!("run {} {}", run.id, current.status.as_str());
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

fn emit_event(e: &StoredEvent, json: bool) -> Result<()> {
    if json {
        use std::io::Write;
        println!("{}", serde_json::to_string(e)?);
        std::io::stdout().flush()?;
    } else {
        print_event(e);
    }
    Ok(())
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
