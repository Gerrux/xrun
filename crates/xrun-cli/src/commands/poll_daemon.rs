#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{
    config::credentials::{KaggleCredentials, VastCredentials},
    manifest::{Manifest, Vendor},
    store::{RunId, RunStatus},
    vendor::InstanceHandle,
    Credentials, GlobalConfig, Store, VendorAdapter,
};
use xrun_kaggle::KaggleAdapter;
use xrun_poller::{mlflow_mirror::MlflowMirrorConfig, CancellationToken, Poller};
use xrun_vast::VastAdapter;

use crate::cli::PollDaemonArgs;

fn load_mlflow_config(
    manifest_path: &Path,
    mlflow_url: Option<&str>,
) -> Option<MlflowMirrorConfig> {
    let url = mlflow_url?;
    let content = std::fs::read_to_string(manifest_path).ok()?;
    let manifest: Manifest = serde_yaml::from_str(&content).ok()?;
    let experiment = manifest
        .mlflow
        .as_ref()
        .and_then(|m| m.experiment.clone())?;
    Some(MlflowMirrorConfig {
        url: url.to_string(),
        experiment,
        auth: None,
        log_args_as_params: manifest
            .mlflow
            .as_ref()
            .and_then(|m| m.log_args_as_params)
            .unwrap_or(true),
        run_name: Some(manifest.name.clone()),
        vendor: match manifest.vendor {
            Vendor::Vast => "vast",
            Vendor::Kaggle => "kaggle",
        }
        .to_string(),
        instance_id: None,
    })
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

    // Reconstruct the vendor adapter based on the run's vendor field.
    let vendor: Box<dyn VendorAdapter> = match run.vendor.as_str() {
        "kaggle" => {
            let kaggle_creds = resolve_kaggle_credentials(config_dir);
            let data_dir = db_path.parent().unwrap_or(db_path);
            let adapter = KaggleAdapter::new()
                .with_store_path(data_dir.to_path_buf())
                .with_credentials(kaggle_creds);
            adapter.set_run_id(&run_id);
            Box::new(adapter)
        }
        _ => {
            let adapter_store = Store::open(db_path)
                .with_context(|| format!("failed to open adapter store at {}", db_path.display()))?;
            let creds = resolve_vast_credentials(config_dir);
            let adapter = VastAdapter::new(creds, adapter_store);
            adapter.set_run_id(&run_id);
            Box::new(adapter)
        }
    };

    let global = GlobalConfig::load(config_dir).unwrap_or_default();
    let budget_cfg = global.budget.clone();
    let cancel = CancellationToken::new();
    let mut poller = Poller::new(
        run_id.clone(),
        store,
        vendor,
        handle,
        runs_dir.to_path_buf(),
    )
    .with_budget(budget_cfg);

    // Wire MLflow mirror from saved manifest (if available and mlflow is configured).
    let manifest_path = runs_dir.join(run_id.to_string()).join("manifest.yaml");
    if let Some(cfg) = load_mlflow_config(&manifest_path, global.mlflow.url.as_deref()) {
        poller = poller.with_mlflow(cfg);
    }

    let result = poller.run(cancel);

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
