#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use xrun_core::{
    config::credentials::VastCredentials,
    store::{Run, RunStatus},
    vendor::InstanceHandle,
    Credentials, RunId, Store, VendorAdapter,
};
use xrun_vast::VastAdapter;

use crate::cli::StopArgs;

pub fn run(args: &StopArgs, db_path: &Path, config_dir: &Path) -> Result<()> {
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    if args.all {
        return stop_all(store, config_dir, args.keep_instance, db_path);
    }

    let id_owned;
    let id: &str = match &args.id {
        Some(id) => id.as_str(),
        None => {
            let active = store.list_active_runs()?;
            match active.len() {
                0 => {
                    println!("no active runs");
                    return Ok(());
                }
                1 => {
                    id_owned = active[0].id.to_string();
                    id_owned.as_str()
                }
                _ => anyhow::bail!(
                    "multiple active runs ({}); pass a run ID or --all",
                    active.len()
                ),
            }
        }
    };

    let parsed: RunId = id
        .parse()
        .with_context(|| format!("invalid run ID: {id}"))?;
    let run = store
        .get_run(&parsed)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {id}"))?;

    stop_one(&run, store, config_dir, args.keep_instance, db_path)?;
    println!("stopped {}", run.id);
    Ok(())
}

fn stop_all(
    store: Store,
    config_dir: &Path,
    keep_instance: bool,
    db_path: &Path,
) -> Result<()> {
    let active = store.list_active_runs()?;
    if active.is_empty() {
        println!("no active runs");
        return Ok(());
    }

    let count = active.len();
    drop(store);

    let mut errors = 0usize;
    for run in active {
        let s = Store::open(db_path)?;
        match stop_one(&run, s, config_dir, keep_instance, db_path) {
            Ok(()) => println!("stopped {}", run.id),
            Err(e) => {
                eprintln!("error: failed to stop {}: {e:#}", run.id);
                errors += 1;
            }
        }
    }

    if errors > 0 {
        anyhow::bail!("{}/{} stop attempts failed", errors, count);
    }
    Ok(())
}

fn stop_one(
    run: &Run,
    mut store: Store,
    config_dir: &Path,
    keep_instance: bool,
    db_path: &Path,
) -> Result<()> {
    if !keep_instance {
        if let Some(instance_id) = &run.instance_id {
            if let Some(instance) = store.get_instance(instance_id)? {
                if instance.destroyed_at.is_none() {
                    if let Some(state_json) = instance.state_json.as_deref() {
                        let handle: InstanceHandle = serde_json::from_str(state_json)
                            .context("failed to deserialize instance handle")?;
                        let adapter_store = Store::open(db_path)?;
                        let creds = resolve_vast_credentials(config_dir);
                        let adapter = build_adapter(&handle.vendor, creds, adapter_store)?;
                        adapter.set_run_id(&run.id);
                        adapter
                            .destroy(&handle)
                            .with_context(|| format!("destroy failed for instance {}", handle.id))?;
                    } else {
                        store.update_instance_destroyed(instance_id, Utc::now())?;
                    }
                }
            }
        }
    }

    store.update_run_status(&run.id, RunStatus::Cancelled)?;
    Ok(())
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

fn build_adapter(
    vendor: &str,
    creds: VastCredentials,
    store: Store,
) -> Result<Box<dyn VendorAdapter>> {
    match vendor {
        "vast" => Ok(Box::new(VastAdapter::new(creds, store))),
        other => anyhow::bail!("stop not implemented for vendor: {other}"),
    }
}
