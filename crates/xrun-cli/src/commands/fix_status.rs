#![deny(unsafe_code)]

//! `xrun fix-status` — reconcile stale `running` runs against the vendor.
//!
//! When a poll-daemon dies mid-run (e.g. the binary was replaced on Windows
//! while the process was running), some runs stay stuck in `running` in the
//! DB forever. This command calls the vendor once per affected run and
//! updates the stored status to match reality.

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{store::RunStatus, vendor::InstanceHandle, Store, VendorAdapter};

use crate::cli::FixStatusArgs;
use crate::commands::common::{build_adapter, open_store, resolve_run, vendor_or_vast, AdapterCtx};

pub fn run(args: &FixStatusArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let store = open_store(db_path)?;

    let runs = if let Some(ref id_str) = args.id {
        vec![resolve_run(&store, id_str)?]
    } else {
        store
            .list_active_runs()?
            .into_iter()
            .filter(|r| r.status == RunStatus::Running)
            .collect()
    };

    if runs.is_empty() {
        println!("no running runs to reconcile");
        return Ok(());
    }

    println!("reconciling {} run(s)…", runs.len());
    let mut changed = 0usize;

    for run in &runs {
        let run_id = &run.id;
        let run_dir = runs_dir.join(run_id.to_string());

        let instance_id = match run.instance_id.as_ref() {
            Some(id) => id.clone(),
            None => {
                eprintln!("  {run_id}: no instance_id — skipping");
                continue;
            }
        };

        let instance = match store.get_instance(&instance_id)? {
            Some(i) => i,
            None => {
                eprintln!("  {run_id}: instance {instance_id} not in DB — skipping");
                continue;
            }
        };

        let state_json = match instance.state_json.as_ref() {
            Some(s) => s.clone(),
            None => {
                eprintln!("  {run_id}: no stored handle — skipping");
                continue;
            }
        };

        let handle: InstanceHandle =
            serde_json::from_str(&state_json).context("failed to deserialize instance handle")?;

        // Unrecognised vendor strings have always been treated as vast here.
        // The kaggle adapter gets MLflow wired so `ingest_telemetry_chunks`
        // (called by `poll_completion`) can backfill events/metrics the dead
        // poller missed before status flipped to Complete; without it the
        // metric tail stays empty for the gap between the poller's last tick
        // and kernel completion.
        let mut ctx = AdapterCtx::new("fix-status", db_path, runs_dir, config_dir, run_id);
        ctx.kaggle_mlflow = true;
        let vendor: Box<dyn VendorAdapter> = match build_adapter(vendor_or_vast(&run.vendor), &ctx)
        {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  {run_id}: {e:#} — skipping");
                continue;
            }
        };

        // Kaggle (and future batch vendors): poll_completion does a one-shot
        // status check and returns a terminal RunStatus when the kernel is done.
        if let Some(result) = vendor.poll_completion(&handle, &run_dir) {
            match result.terminal_status {
                Some(terminal) => {
                    println!(
                        "  {run_id} [{vendor}]: running → {s}",
                        vendor = run.vendor,
                        s = terminal.as_str(),
                    );
                    if !args.dry_run {
                        let mut w = Store::open(db_path)?;
                        w.update_run_status(run_id, terminal)?;
                    }
                    changed += 1;
                }
                None => {
                    println!(
                        "  {run_id} [{vendor}]: still running (vendor confirms)",
                        vendor = run.vendor,
                    );
                }
            }
            continue;
        }

        // vast (and SSH-based vendors): check whether the instance still
        // appears in the vendor's live list. If it's gone, the run ended
        // without the daemon catching the final status — mark failed so the
        // user can inspect logs and re-run.
        match vendor.vendor_instances() {
            Ok(remote) => {
                let alive = remote.iter().any(|r| r.id == instance_id);
                if alive {
                    println!(
                        "  {run_id} [{vendor}]: instance {instance_id} still alive — no change",
                        vendor = run.vendor,
                    );
                } else {
                    println!(
                        "  {run_id} [{vendor}]: instance {instance_id} gone from vendor — marking failed",
                        vendor = run.vendor,
                    );
                    if !args.dry_run {
                        let mut w = Store::open(db_path)?;
                        w.update_run_status(run_id, RunStatus::Failed)?;
                    }
                    changed += 1;
                }
            }
            Err(e) => {
                eprintln!(
                    "  {run_id} [{vendor}]: could not query vendor instances: {e}",
                    vendor = run.vendor,
                );
            }
        }
    }

    if args.dry_run {
        println!("(dry run — no changes written)");
    } else {
        println!("{changed} run(s) updated");
    }

    Ok(())
}
