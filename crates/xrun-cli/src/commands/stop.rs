#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{
    store::{Run, RunStatus},
    vendor::InstanceHandle,
    Store,
};

use crate::cli::StopArgs;
use crate::commands::common::{build_adapter, open_store, select_run, AdapterCtx, RunSelection};

pub fn run(args: &StopArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let store = open_store(db_path)?;

    if args.all {
        return stop_all(store, runs_dir, config_dir, args.keep_instance, db_path);
    }

    let run = match select_run(&store, args.id.as_deref())? {
        RunSelection::Run(run) => *run,
        RunSelection::NoActive => {
            println!("no active runs");
            return Ok(());
        }
        RunSelection::Multiple(n) => {
            anyhow::bail!("multiple active runs ({n}); pass a run ID or --all")
        }
    };

    match stop_one(
        &run,
        store,
        runs_dir,
        config_dir,
        args.keep_instance,
        db_path,
    )? {
        StopOutcome::Cancelled => println!("stopped {}", run.id),
        StopOutcome::Released => println!("released instance for finished run {}", run.id),
        StopOutcome::NothingToRelease => println!(
            "run {} already {}; no live instance to release",
            run.id,
            run.status.as_str()
        ),
    }
    Ok(())
}

/// What `stop_one` did to the run.
#[derive(Debug, PartialEq, Eq)]
enum StopOutcome {
    /// An active run: the instance (if any) was destroyed and the run is now
    /// `cancelled`.
    Cancelled,
    /// A run already in a terminal status (`policy.on_done: keep`, a failed
    /// run with the instance left for debugging): the instance was destroyed
    /// and the status was left as-is.
    Released,
    /// A terminal run with no live instance: nothing to do, status untouched.
    NothingToRelease,
}

fn stop_all(
    store: Store,
    runs_dir: &Path,
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
        let s = open_store(db_path)?;
        // `active` holds non-terminal runs only, so every success is a cancel.
        match stop_one(&run, s, runs_dir, config_dir, keep_instance, db_path) {
            Ok(_) => println!("stopped {}", run.id),
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
    runs_dir: &Path,
    config_dir: &Path,
    keep_instance: bool,
    db_path: &Path,
) -> Result<StopOutcome> {
    if keep_instance {
        anyhow::bail!("--keep-instance cannot safely stop the training process yet; run remains unchanged. Omit this flag to stop and release the run resource.");
    }
    let mut destroyed = false;
    if let Some(instance_id) = &run.instance_id {
        if let Some(instance) = store.get_instance(instance_id)? {
            if instance.destroyed_at.is_none() {
                if let Some(state_json) = instance.state_json.as_deref() {
                    let handle: InstanceHandle = serde_json::from_str(state_json)
                        .context("failed to deserialize instance handle")?;
                    let mut ctx = AdapterCtx::new("stop", db_path, runs_dir, config_dir, &run.id);
                    // Kaggle destroy goes through REST; it needs neither
                    // the data dir nor MLflow.
                    ctx.kaggle_store_path = false;
                    let adapter = build_adapter(&handle.vendor, &ctx)?;
                    adapter
                        .destroy(&handle)
                        .with_context(|| format!("destroy failed for instance {}", handle.id))?;
                    destroyed = true;
                } else {
                    anyhow::bail!("instance {instance_id} has no saved handle; cannot verify resource cleanup");
                }
            }
        }
    }

    // A run that already finished (`done` under `policy.on_done: keep`, or
    // `failed` with the instance left for debugging) is not being cancelled:
    // `xrun stop` only releases its instance. Rewriting the status here
    // misreported successful runs as `cancelled`.
    if run.status.is_terminal() {
        return Ok(if destroyed {
            StopOutcome::Released
        } else {
            StopOutcome::NothingToRelease
        });
    }

    store.update_run_status(&run.id, RunStatus::Cancelled)?;
    Ok(StopOutcome::Cancelled)
}
