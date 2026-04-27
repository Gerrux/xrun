#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{RunId, Store};

use crate::cli::ShowArgs;

pub fn run(args: &ShowArgs, db_path: &Path, runs_dir: &Path) -> Result<()> {
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

    let events = store.list_events(&id).context("failed to list events")?;
    let metric_keys = store
        .list_metric_keys(&id)
        .context("failed to list metric keys")?;

    if args.json {
        let out = serde_json::json!({
            "run": run,
            "events": events,
            "metric_keys": metric_keys.iter().map(|(k, c)| serde_json::json!({"key": k, "count": c})).collect::<Vec<_>>(),
        });
        println!("{out}");
    } else {
        println!("Run: {}", run.id);
        println!("  name:          {}", run.name);
        println!("  status:        {}", run.status.as_str());
        println!("  vendor:        {}", run.vendor);
        println!("  manifest_hash: {}", &run.manifest_hash[..16]);
        println!("  manifest_path: {}", run.manifest_path);
        println!(
            "  created_at:    {}",
            run.created_at.format("%Y-%m-%dT%H:%M:%SZ")
        );
        println!(
            "  started_at:    {}",
            run.started_at
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "  ended_at:      {}",
            run.ended_at
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "  cost_usd:      {}",
            run.cost_usd
                .map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "-".to_string())
        );
        println!();
        let run_dir = runs_dir.join(run.id.to_string());
        println!("  run_dir: {}", run_dir.display());
        println!();
        println!("Events ({}):", events.len());
        if events.is_empty() {
            println!("  (none)");
        } else {
            println!("  {:<24}  {:<20}  {:<8}  msg", "ts", "stage", "status");
            for e in &events {
                println!(
                    "  {:<24}  {:<20}  {:<8}  {}",
                    e.ts.format("%Y-%m-%dT%H:%M:%SZ"),
                    e.stage,
                    e.status,
                    e.msg.as_deref().unwrap_or("")
                );
            }
        }
        println!();
        println!("Metric keys ({}):", metric_keys.len());
        if metric_keys.is_empty() {
            println!("  (none)");
        } else {
            for (k, c) in &metric_keys {
                println!("  {k}: {c} points");
            }
        }
    }

    Ok(())
}
