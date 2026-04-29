#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{
    config::credentials::VastCredentials,
    store::{RunId, RunStatus},
    vendor::InstanceHandle,
    Credentials, Store, VendorAdapter,
};
use xrun_poller::{CancellationToken, Poller};
use xrun_vast::VastAdapter;

use crate::cli::PollDaemonArgs;

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

/// Run the poller daemon for an existing run.
///
/// Called by `xrun __poll-daemon <run-id>` when a run is launched with `--detach`.
/// Reconstructs the VendorAdapter and InstanceHandle from the DB, then runs the
/// polling loop until the run completes, fails, or is cancelled.
pub fn run(
    args: &PollDaemonArgs,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
) -> Result<()> {
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

    let state_json = instance.state_json.ok_or_else(|| {
        anyhow::anyhow!(
            "instance {} has no stored handle state; re-launch without --detach",
            instance_id
        )
    })?;

    let handle: InstanceHandle =
        serde_json::from_str(&state_json).context("failed to deserialize instance handle")?;

    // Reconstruct the vendor adapter for the poller (only needs tail/destroy).
    let adapter_store = Store::open(db_path)
        .with_context(|| format!("failed to open adapter store at {}", db_path.display()))?;
    let creds = resolve_vast_credentials(config_dir);
    let adapter = VastAdapter::new(creds, adapter_store);
    adapter.set_run_id(&run_id);
    let vendor: Box<dyn VendorAdapter> = Box::new(adapter);

    let cancel = CancellationToken::new();
    let result = Poller::new(
        run_id.clone(),
        store,
        vendor,
        handle,
        runs_dir.to_path_buf(),
    )
    .run(cancel);

    match result {
        Ok(RunStatus::Done) => {
            tracing::info!("poll-daemon: run {run_id} completed");
            Ok(())
        }
        Ok(RunStatus::Failed) => {
            tracing::warn!("poll-daemon: run {run_id} failed");
            Ok(())
        }
        Ok(RunStatus::Cancelled) => {
            tracing::info!("poll-daemon: run {run_id} was cancelled");
            Ok(())
        }
        Ok(s) => {
            tracing::warn!("poll-daemon: run {run_id} ended with status {}", s.as_str());
            Ok(())
        }
        Err(e) => anyhow::bail!("poll-daemon error for run {run_id}: {e}"),
    }
}
