#![deny(unsafe_code)]

//! `xrun fix-status` — reconcile stale `running` runs against the vendor.
//!
//! When a poll-daemon dies mid-run (e.g. the binary was replaced on Windows
//! while the process was running), some runs stay stuck in `running` in the
//! DB forever. This command calls the vendor once per affected run and
//! updates the stored status to match reality.

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{
    config::credentials::{KaggleCredentials, VastCredentials},
    store::{RunId, RunStatus},
    vendor::InstanceHandle,
    Credentials, Store, VendorAdapter,
};
use xrun_kaggle::KaggleAdapter;
use xrun_vast::VastAdapter;

use crate::cli::FixStatusArgs;

pub fn run(args: &FixStatusArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
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

        let handle: InstanceHandle = serde_json::from_str(&state_json)
            .context("failed to deserialize instance handle")?;

        let vendor: Box<dyn VendorAdapter> = match run.vendor.as_str() {
            "kaggle" => {
                let creds = resolve_kaggle_credentials(config_dir);
                let data_dir = db_path.parent().unwrap_or(db_path);
                let adapter = KaggleAdapter::new()
                    .with_store_path(data_dir.to_path_buf())
                    .with_credentials(creds);
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

fn resolve_vast_credentials(config_dir: &Path) -> VastCredentials {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.vast.api_key.is_some() {
            return creds.vast;
        }
    }
    if let Ok(Some(token)) = Credentials::import_vast_native() {
        return VastCredentials { api_key: Some(token) };
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
