#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{
    config::credentials::{KaggleCredentials, VastCredentials},
    store::{Run, RunStatus},
    vendor::InstanceHandle,
    Credentials, RunId, Store, VendorAdapter,
};
use xrun_kaggle::KaggleAdapter;
use xrun_local::LocalAdapter;
use xrun_ssh::SshAdapter;
use xrun_vast::VastAdapter;

use crate::cli::StopArgs;

pub fn run(args: &StopArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    if args.all {
        return stop_all(store, runs_dir, config_dir, args.keep_instance, db_path);
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
        let s = Store::open(db_path)?;
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
                        let adapter_store = Store::open(db_path)?;
                        let adapter = build_adapter(
                            &handle.vendor,
                            runs_dir,
                            config_dir,
                            adapter_store,
                            &run.id,
                        )?;
                        adapter.set_run_id(&run.id);
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

fn build_adapter(
    vendor: &str,
    runs_dir: &Path,
    config_dir: &Path,
    store: Store,
    run_id: &RunId,
) -> Result<Box<dyn VendorAdapter>> {
    match vendor {
        "vast" => {
            let creds = resolve_vast_credentials(config_dir);
            Ok(Box::new(VastAdapter::new(creds, store)))
        }
        "kaggle" => {
            let creds = resolve_kaggle_credentials(config_dir);
            // KaggleAdapter destroys via REST and doesn't need the store
            // — drop it so we don't hold an extra connection.
            drop(store);
            Ok(Box::new(KaggleAdapter::new().with_credentials(creds)))
        }
        "local" => Ok(Box::new(LocalAdapter::with_store_and_runs_dir(
            store,
            runs_dir.to_path_buf(),
        ))),
        "ssh" => {
            let creds = Credentials::load(config_dir).unwrap_or_default();
            let manifest_path = runs_dir.join(run_id.to_string()).join("manifest.yaml");
            let yaml = std::fs::read_to_string(&manifest_path)
                .context("stop: cannot read saved SSH manifest")?;
            let manifest = xrun_core::manifest::Manifest::from_yaml_str(&yaml)?;
            let ssh = manifest
                .ssh
                .as_ref()
                .context("stop: saved manifest has no ssh configuration")?;
            let alias = &ssh.host_alias;
            let host_creds = creds
                .ssh_hosts
                .get(alias)
                .ok_or_else(|| anyhow::anyhow!("stop: ssh alias '{alias}' missing"))?;
            let conn = SshAdapter::resolve_conn(alias, host_creds)?;
            let workdir_root = ssh
                .workdir
                .clone()
                .or_else(|| host_creds.default_workdir.clone())
                .unwrap_or_else(|| "/tmp/xrun".to_string());
            Ok(Box::new(SshAdapter::new(store, conn, workdir_root)))
        }
        other => anyhow::bail!("stop not implemented for vendor: {other}"),
    }
}
