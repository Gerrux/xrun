#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{ListFilter, RunStatus, Store};

use crate::cli::LsArgs;

pub fn run(args: &LsArgs, db_path: &Path) -> Result<()> {
    if args.manifests {
        println!("not implemented for v0.1");
        return Ok(());
    }

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let status_filter = args.status.as_deref().map(parse_status).transpose()?;

    let filter = ListFilter {
        status: status_filter,
        vendor: args.vendor.clone(),
    };

    let mut runs = store.list_runs(&filter).context("failed to list runs")?;

    if !args.all {
        let mut done_count = 0;
        runs.retain(|r| {
            if matches!(
                r.status,
                RunStatus::Provisioning | RunStatus::Uploading | RunStatus::Running
            ) {
                return true;
            }
            if done_count < 10 {
                done_count += 1;
                return true;
            }
            false
        });
    }

    if let Some(tag) = &args.tag {
        runs.retain(|r| {
            r.notes
                .as_deref()
                .and_then(|n| serde_json::from_str::<Vec<String>>(n).ok())
                .map(|tags| tags.contains(tag))
                .unwrap_or(false)
        });
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string(&runs).unwrap_or_else(|_| "[]".to_string())
        );
    } else if runs.is_empty() {
        println!("no runs found");
    } else {
        println!(
            "{:<26}  {:<16}  {:<12}  {:<8}  created_at",
            "id", "name", "status", "vendor"
        );
        println!("{}", "-".repeat(80));
        for r in &runs {
            println!(
                "{:<26}  {:<16}  {:<12}  {:<8}  {}",
                r.id,
                truncate(&r.name, 16),
                r.status.as_str(),
                r.vendor,
                r.created_at.format("%Y-%m-%dT%H:%M:%SZ")
            );
        }
    }

    Ok(())
}

fn parse_status(s: &str) -> Result<RunStatus> {
    match s {
        "provisioning" => Ok(RunStatus::Provisioning),
        "uploading" => Ok(RunStatus::Uploading),
        "running" => Ok(RunStatus::Running),
        "done" => Ok(RunStatus::Done),
        "failed" => Ok(RunStatus::Failed),
        "cancelled" => Ok(RunStatus::Cancelled),
        other => anyhow::bail!("unknown status: {other}"),
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        None => s,
        Some((i, _)) => &s[..i],
    }
}
