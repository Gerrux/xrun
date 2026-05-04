#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{manifest::Manifest, paths, vendor::InstanceHandle, RunId, Store};
use xrun_kaggle::snapshot;

use crate::cli::ShowArgs;

struct DatasetSummary {
    slug: String,
    files: usize,
    total_bytes: u64,
    captured_at: String,
}

fn dataset_summary_for(run_dir: &Path) -> Option<DatasetSummary> {
    let manifest_path = run_dir.join("manifest.yaml");
    let yaml = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest = Manifest::from_yaml_str(&yaml).ok()?;
    let kaggle = manifest.kaggle.as_ref()?;
    let slug = kaggle.dataset.as_deref()?;
    let snapshots_dir = paths::data_dir().ok()?.join("dataset_snapshots");
    let snap = snapshot::load(&snapshots_dir, slug)?;
    let total_bytes: u64 = snap.files.values().map(|e| e.size).sum();
    Some(DatasetSummary {
        slug: snap.slug,
        files: snap.files.len(),
        total_bytes,
        captured_at: snap.captured_at,
    })
}

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

    // Pull ssh_host/port from the instance handle persisted at provision time.
    // Active runs hit this constantly during debugging — without it every
    // `vastai ssh` command starts with a `xrun show | grep ssh` round-trip.
    let ssh = run
        .instance_id
        .as_deref()
        .and_then(|iid| store.get_instance(iid).ok().flatten())
        .and_then(|inst| inst.state_json)
        .and_then(|s| serde_json::from_str::<InstanceHandle>(&s).ok())
        .and_then(|h| match (h.ssh_host, h.ssh_port) {
            (Some(host), Some(port)) if !host.is_empty() => Some((host, port, h.ssh_user)),
            _ => None,
        });

    let run_dir = runs_dir.join(run.id.to_string());
    let dataset = dataset_summary_for(&run_dir);

    if args.json {
        let ssh_json = ssh.as_ref().map(|(h, p, u)| {
            serde_json::json!({
                "host": h, "port": p, "user": u, "command": format!("ssh -p {p} {u}@{h}")
            })
        });
        let dataset_json = dataset.as_ref().map(|d| {
            serde_json::json!({
                "slug": d.slug,
                "files": d.files,
                "total_bytes": d.total_bytes,
                "last_pushed_at": d.captured_at,
            })
        });
        let out = serde_json::json!({
            "run": run,
            "events": events,
            "metric_keys": metric_keys.iter().map(|(k, c)| serde_json::json!({"key": k, "count": c})).collect::<Vec<_>>(),
            "ssh": ssh_json,
            "dataset": dataset_json,
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
        if let Some((host, port, user)) = &ssh {
            println!("  ssh:           {user}@{host}:{port}");
            println!("  ssh_command:   ssh -p {port} {user}@{host}");
        }
        if let Some(d) = &dataset {
            let mb = d.total_bytes as f64 / (1024.0 * 1024.0);
            println!(
                "  dataset:       {} ({} files, {:.1} MiB; last push {})",
                d.slug, d.files, mb, d.captured_at
            );
        }
        println!();
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
