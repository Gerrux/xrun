#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{RunId, Store};

use crate::cli::MetricsArgs;

pub fn run(args: &MetricsArgs, db_path: &Path) -> Result<()> {
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

    let filter_keys: Option<Vec<String>> = args
        .key
        .as_deref()
        .map(|s| s.split(',').map(str::trim).map(str::to_string).collect());

    if args.ascii {
        println!("no data yet");
        return Ok(());
    }

    if let Some(keys) = &filter_keys {
        let metrics = store
            .list_metrics(&run.id, Some(keys))
            .context("failed to list metrics")?;

        if args.json {
            println!(
                "{}",
                serde_json::to_string(&metrics).unwrap_or_else(|_| "[]".to_string())
            );
        } else if metrics.is_empty() {
            println!("no metrics found for keys: {}", keys.join(", "));
        } else {
            println!("{:<8}  {:<30}  {:<12}  ts", "step", "key", "value");
            println!("{}", "-".repeat(70));
            for m in &metrics {
                println!(
                    "{:<8}  {:<30}  {:<12.6}  {}",
                    m.step,
                    m.key,
                    m.value,
                    m.ts.format("%Y-%m-%dT%H:%M:%SZ")
                );
            }
        }
    } else {
        let keys = store
            .list_metric_keys(&run.id)
            .context("failed to list metric keys")?;

        if args.json {
            let out: Vec<_> = keys
                .iter()
                .map(|(k, c)| serde_json::json!({"key": k, "count": c}))
                .collect();
            println!(
                "{}",
                serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
            );
        } else if keys.is_empty() {
            println!("no metrics for run {}", args.id);
        } else {
            println!("Available metric keys:");
            for (k, c) in &keys {
                println!("  {k}: {c} points");
            }
        }
    }

    Ok(())
}
