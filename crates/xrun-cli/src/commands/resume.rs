#![deny(unsafe_code)]

//! `xrun resume` — re-attach poll-daemons to runs that lost theirs.
//!
//! Use case: power loss, OS reboot, or user-killed `xrun launch --detach`
//! parent left a run with `status=running` in the DB but no live poller.
//! For each affected run:
//!
//! 1. If the recorded `poller_pid` is still alive, skip — already running.
//! 2. Otherwise probe the vendor for the instance's liveness.
//! 3. If the instance is still up, spawn a fresh `__poll-daemon` and record
//!    the new PID. The poller resumes from `poll_offsets` and continues
//!    streaming events / metrics / stdout into SQLite.
//! 4. If the instance is gone, fall back to the same reconcile logic as
//!    `xrun fix-status` (mark Failed, or use `poll_completion`'s terminal
//!    status for batch vendors).

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use xrun_core::{
    store::{Run, RunStatus},
    vendor::InstanceHandle,
    Store, VendorAdapter,
};
use xrun_local::process::process_alive;

use crate::cli::ResumeArgs;
use crate::commands::common::{build_adapter, open_store, resolve_run, vendor_or_vast, AdapterCtx};
use crate::commands::launch::spawn_daemon;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Recorded poller PID is still alive — nothing to do.
    AlreadyRunning,
    /// Vendor instance still up — spawned a new poll-daemon.
    Respawned,
    /// Vendor reports the instance is gone — run marked terminal.
    Reconciled,
    /// Could not act (missing instance handle, vendor probe failed, etc.).
    Skipped,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub run_id: String,
    pub vendor: String,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poller_pid: Option<i64>,
    /// Set on Reconciled outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_status: Option<String>,
    /// Human-readable reason for Skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub fn run(args: &ResumeArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
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

    let mut reports: Vec<Report> = Vec::with_capacity(runs.len());
    for run in &runs {
        match resume_one(run, db_path, runs_dir, config_dir, args.dry_run) {
            Ok(r) => reports.push(r),
            Err(e) => reports.push(Report {
                run_id: run.id.to_string(),
                vendor: run.vendor.clone(),
                outcome: Outcome::Skipped,
                poller_pid: None,
                final_status: None,
                note: Some(e.to_string()),
            }),
        }
    }

    if args.json {
        let out = serde_json::json!({ "runs": reports });
        println!("{out}");
    } else {
        if reports.is_empty() {
            println!("no running runs to resume");
            return Ok(());
        }
        for r in &reports {
            let extra = match &r.outcome {
                Outcome::AlreadyRunning => {
                    format!("already running (pid {})", r.poller_pid.unwrap_or_default())
                }
                Outcome::Respawned => format!(
                    "respawned poller (pid {})",
                    r.poller_pid.unwrap_or_default()
                ),
                Outcome::Reconciled => format!(
                    "reconciled → {}",
                    r.final_status.as_deref().unwrap_or("unknown")
                ),
                Outcome::Skipped => {
                    format!("skipped: {}", r.note.as_deref().unwrap_or("no reason"))
                }
            };
            println!("  {} [{}]: {}", r.run_id, r.vendor, extra);
        }
        if args.dry_run {
            println!("(dry run — no changes written)");
        }
    }
    Ok(())
}

pub(crate) fn resume_one(
    run: &Run,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
    dry_run: bool,
) -> Result<Report> {
    let run_id = &run.id;
    let run_dir = runs_dir.join(run_id.to_string());

    // Step 1: liveness via stored PID. Avoids any vendor I/O for the common case.
    if let Some(pid) = run.poller_pid {
        if pid > 0 && pid <= u32::MAX as i64 && process_alive(pid as u32) {
            return Ok(Report {
                run_id: run_id.to_string(),
                vendor: run.vendor.clone(),
                outcome: Outcome::AlreadyRunning,
                poller_pid: Some(pid),
                final_status: None,
                note: None,
            });
        }
    }

    let instance_id = match run.instance_id.as_ref() {
        Some(id) => id.clone(),
        None => {
            return Ok(Report {
                run_id: run_id.to_string(),
                vendor: run.vendor.clone(),
                outcome: Outcome::Skipped,
                poller_pid: None,
                final_status: None,
                note: Some("no instance_id".into()),
            });
        }
    };

    let store_ro = Store::open(db_path)?;
    let instance = match store_ro.get_instance(&instance_id)? {
        Some(i) => i,
        None => {
            return Ok(Report {
                run_id: run_id.to_string(),
                vendor: run.vendor.clone(),
                outcome: Outcome::Skipped,
                poller_pid: None,
                final_status: None,
                note: Some(format!("instance {instance_id} not in DB")),
            });
        }
    };

    let state_json = match instance.state_json.as_ref() {
        Some(s) => s.clone(),
        None => {
            return Ok(Report {
                run_id: run_id.to_string(),
                vendor: run.vendor.clone(),
                outcome: Outcome::Skipped,
                poller_pid: None,
                final_status: None,
                note: Some("instance has no stored handle".into()),
            });
        }
    };
    let handle: InstanceHandle =
        serde_json::from_str(&state_json).context("failed to deserialize instance handle")?;

    let vendor = match build_vendor(run, db_path, runs_dir, config_dir)? {
        Some(v) => v,
        None => {
            return Ok(Report {
                run_id: run_id.to_string(),
                vendor: run.vendor.clone(),
                outcome: Outcome::Skipped,
                poller_pid: None,
                final_status: None,
                note: Some("vendor reconstruction failed".into()),
            });
        }
    };

    // Step 2a: batch-style vendors (Kaggle) expose terminal status directly.
    if let Some(result) = vendor.poll_completion(&handle, &run_dir) {
        if let Some(terminal) = result.terminal_status {
            if !dry_run {
                let mut w = Store::open(db_path)?;
                w.update_run_status(run_id, terminal.clone())?;
                let _ = w.update_run_poller_pid(run_id, None);
            }
            return Ok(Report {
                run_id: run_id.to_string(),
                vendor: run.vendor.clone(),
                outcome: Outcome::Reconciled,
                poller_pid: None,
                final_status: Some(terminal.as_str().to_string()),
                note: None,
            });
        }
        // Kernel still running — respawn poller so it keeps ingesting.
        return respawn(run, db_path, runs_dir, config_dir, dry_run);
    }

    // Step 2b: SSH-style vendors (vast, ssh, local) — check the live list.
    match vendor.vendor_instances() {
        Ok(remote) => {
            let alive = remote.iter().any(|r| r.id == instance_id);
            if alive {
                respawn(run, db_path, runs_dir, config_dir, dry_run)
            } else {
                if !dry_run {
                    let mut w = Store::open(db_path)?;
                    w.update_run_status(run_id, RunStatus::Failed)?;
                    let _ = w.update_run_poller_pid(run_id, None);
                }
                Ok(Report {
                    run_id: run_id.to_string(),
                    vendor: run.vendor.clone(),
                    outcome: Outcome::Reconciled,
                    poller_pid: None,
                    final_status: Some(RunStatus::Failed.as_str().to_string()),
                    note: Some(format!("instance {instance_id} gone from vendor")),
                })
            }
        }
        Err(e) => Ok(Report {
            run_id: run_id.to_string(),
            vendor: run.vendor.clone(),
            outcome: Outcome::Skipped,
            poller_pid: None,
            final_status: None,
            note: Some(format!("vendor probe failed: {e}")),
        }),
    }
}

fn respawn(
    run: &Run,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
    dry_run: bool,
) -> Result<Report> {
    if dry_run {
        return Ok(Report {
            run_id: run.id.to_string(),
            vendor: run.vendor.clone(),
            outcome: Outcome::Respawned,
            poller_pid: None,
            final_status: None,
            note: Some("dry-run: would spawn poll-daemon".into()),
        });
    }
    let pid = spawn_daemon(&run.id, db_path, runs_dir, config_dir)?;
    let mut w = Store::open(db_path)?;
    if let Err(e) = w.update_run_poller_pid(&run.id, Some(pid as i64)) {
        tracing::warn!("respawn: could not record poller PID: {e}");
    }
    Ok(Report {
        run_id: run.id.to_string(),
        vendor: run.vendor.clone(),
        outcome: Outcome::Respawned,
        poller_pid: Some(pid as i64),
        final_status: None,
        note: None,
    })
}

/// Build a vendor adapter for an existing run. Returns Ok(None) when the
/// vendor needs config we don't have (e.g. ssh creds missing).
fn build_vendor(
    run: &Run,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
) -> Result<Option<Box<dyn VendorAdapter>>> {
    let ctx = AdapterCtx::new("resume", db_path, runs_dir, config_dir, &run.id);
    // An unrecognised vendor string has always been treated as vast here.
    // Any construction failure (ssh manifest / creds missing, store busy)
    // means "cannot reconstruct", reported as a skip by the caller.
    Ok(build_adapter(vendor_or_vast(&run.vendor), &ctx).ok())
}
