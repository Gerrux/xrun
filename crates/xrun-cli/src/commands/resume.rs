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
    config::credentials::{KaggleCredentials, VastCredentials},
    store::{Run, RunId, RunStatus},
    vendor::InstanceHandle,
    Credentials, Store, VendorAdapter,
};
use xrun_kaggle::KaggleAdapter;
use xrun_local::{process::process_alive, LocalAdapter};
use xrun_ssh::SshAdapter;
use xrun_vast::VastAdapter;

use crate::cli::ResumeArgs;
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
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let runs = if let Some(ref id_str) = args.id {
        let run_id: RunId = id_str
            .parse()
            .with_context(|| format!("invalid run ID: {id_str}"))?;
        let run = store
            .get_run(&run_id)?
            .ok_or_else(|| anyhow::anyhow!("run not found: {id_str}"))?;
        vec![run]
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

fn resume_one(
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
        return respawn(run, db_path, runs_dir, dry_run);
    }

    // Step 2b: SSH-style vendors (vast, ssh, local) — check the live list.
    match vendor.vendor_instances() {
        Ok(remote) => {
            let alive = remote.iter().any(|r| r.id == instance_id);
            if alive {
                respawn(run, db_path, runs_dir, dry_run)
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

fn respawn(run: &Run, db_path: &Path, runs_dir: &Path, dry_run: bool) -> Result<Report> {
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
    let pid = spawn_daemon(&run.id, db_path, runs_dir)?;
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
    let run_id = &run.id;
    let v: Box<dyn VendorAdapter> = match run.vendor.as_str() {
        "kaggle" => {
            let creds = resolve_kaggle_credentials(config_dir);
            let data_dir = db_path.parent().unwrap_or(db_path);
            let adapter = KaggleAdapter::new()
                .with_store_path(data_dir.to_path_buf())
                .with_credentials(creds);
            adapter.set_run_id(run_id);
            Box::new(adapter)
        }
        "local" => {
            let adapter_store = Store::open(db_path)?;
            let adapter =
                LocalAdapter::with_store_and_runs_dir(adapter_store, runs_dir.to_path_buf());
            adapter.set_run_id(run_id);
            Box::new(adapter)
        }
        "ssh" => {
            let manifest_path = runs_dir.join(run_id.to_string()).join("manifest.yaml");
            let yaml = match std::fs::read_to_string(&manifest_path) {
                Ok(y) => y,
                Err(_) => return Ok(None),
            };
            let m: xrun_core::manifest::Manifest = match serde_yaml::from_str(&yaml) {
                Ok(m) => m,
                Err(_) => return Ok(None),
            };
            let ssh_spec = match m.ssh.as_ref() {
                Some(s) => s,
                None => return Ok(None),
            };
            let creds = Credentials::load(config_dir).unwrap_or_default();
            let host_creds = match creds.ssh_hosts.get(&ssh_spec.host_alias) {
                Some(h) => h,
                None => return Ok(None),
            };
            let conn = match SshAdapter::resolve_conn(&ssh_spec.host_alias, host_creds) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            let workdir_root = ssh_spec
                .workdir
                .clone()
                .or_else(|| host_creds.default_workdir.clone())
                .unwrap_or_else(|| "/tmp/xrun".to_string());
            let adapter_store = Store::open(db_path)?;
            let adapter = SshAdapter::new(adapter_store, conn, workdir_root);
            adapter.set_run_id(run_id);
            Box::new(adapter)
        }
        _ => {
            let creds = resolve_vast_credentials(config_dir);
            let adapter_store = Store::open(db_path)?;
            let adapter = VastAdapter::new(creds, adapter_store);
            adapter.set_run_id(run_id);
            Box::new(adapter)
        }
    };
    Ok(Some(v))
}

fn resolve_vast_credentials(config_dir: &Path) -> VastCredentials {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.vast.api_key.is_some() {
            return creds.vast;
        }
    }
    if let Ok(Some(token)) = Credentials::import_vast_native() {
        return VastCredentials {
            api_key: Some(token),
        };
    }
    VastCredentials::default()
}

fn resolve_kaggle_credentials(config_dir: &Path) -> KaggleCredentials {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.kaggle.token.is_some()
            || (creds.kaggle.username.is_some() && creds.kaggle.key.is_some())
        {
            return creds.kaggle;
        }
    }
    if let Ok(Some((username, key))) = Credentials::import_kaggle_native() {
        return KaggleCredentials {
            token: None,
            username: Some(username),
            key: Some(key),
        };
    }
    if let Ok(Some(token)) = Credentials::import_kaggle_access_token() {
        return KaggleCredentials {
            token: Some(token),
            username: None,
            key: None,
        };
    }
    KaggleCredentials::default()
}
