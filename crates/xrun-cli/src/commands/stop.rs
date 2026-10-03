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

    stop_one(
        &run,
        store,
        runs_dir,
        config_dir,
        args.keep_instance,
        db_path,
    )?;
    println!("stopped {}", run.id);
    Ok(())
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
        match stop_one(&run, s, runs_dir, config_dir, keep_instance, db_path) {
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
    runs_dir: &Path,
    config_dir: &Path,
    keep_instance: bool,
    db_path: &Path,
) -> Result<()> {
    if keep_instance {
        anyhow::bail!("--keep-instance cannot safely stop the training process yet; run remains unchanged. Omit this flag to stop and release the run resource.");
    }
    if !keep_instance {
        if let Some(instance_id) = &run.instance_id {
            if let Some(instance) = store.get_instance(instance_id)? {
                if instance.destroyed_at.is_none() {
                    if let Some(state_json) = instance.state_json.as_deref() {
                        let handle: InstanceHandle = serde_json::from_str(state_json)
                            .context("failed to deserialize instance handle")?;
                        let mut ctx =
                            AdapterCtx::new("stop", db_path, runs_dir, config_dir, &run.id);
                        // Kaggle destroy goes through REST; it needs neither
                        // the data dir nor MLflow.
                        ctx.kaggle_store_path = false;
                        let adapter = build_adapter(&handle.vendor, &ctx)?;
                        adapter.destroy(&handle).with_context(|| {
                            format!("destroy failed for instance {}", handle.id)
                        })?;
                    } else {
                        anyhow::bail!("instance {instance_id} has no saved handle; cannot verify resource cleanup");
                    }
                }
            }
        }
    }

    store.update_run_status(&run.id, RunStatus::Cancelled)?;
    Ok(())
}
