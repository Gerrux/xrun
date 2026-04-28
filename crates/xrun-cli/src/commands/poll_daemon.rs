#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::Store;

use crate::cli::PollDaemonArgs;

/// Run the poller daemon for an existing run.
///
/// Called by `xrun __poll-daemon <run-id>` when a run is launched with `--detach`.
/// The full vendor reconstruction is wired up in task 7; this function validates
/// that the run and instance exist and returns an error until that task is complete.
pub fn run(args: &PollDaemonArgs, db_path: &Path, _runs_dir: &Path) -> Result<()> {
    use xrun_core::store::RunId;

    let run_id: RunId = args
        .run_id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.run_id))?;

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let run = store
        .get_run(&run_id)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", args.run_id))?;

    let instance_id = run
        .instance_id
        .ok_or_else(|| anyhow::anyhow!("run {} has no associated instance", args.run_id))?;

    let instance = store
        .get_instance(&instance_id)?
        .ok_or_else(|| anyhow::anyhow!("instance {} not found", instance_id))?;

    if instance.state_json.is_none() {
        anyhow::bail!(
            "instance {} has no stored handle state; \
             re-launch without --detach or upgrade to task-7 implementation",
            instance_id
        );
    }

    // Vendor adapter reconstruction and Poller wiring are completed in task 7.
    anyhow::bail!(
        "poll-daemon: vendor adapter reconstruction not yet implemented \
         (run ID: {run_id}, instance: {instance_id})"
    )
}
