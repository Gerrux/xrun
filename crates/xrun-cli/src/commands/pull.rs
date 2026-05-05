#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use xrun_core::{
    config::credentials::{KaggleCredentials, VastCredentials},
    store::Run,
    vendor::InstanceHandle,
    Credentials, RunId, Store, VendorAdapter,
};
use xrun_kaggle::KaggleAdapter;
use xrun_local::LocalAdapter;
use xrun_ssh::SshAdapter;
use xrun_vast::VastAdapter;

use crate::cli::PullArgs;

pub fn run(args: &PullArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let id_owned;
    let id: &str = match &args.id {
        Some(id) => id.as_str(),
        None => {
            let active = store.list_active_runs()?;
            match active.len() {
                0 => {
                    println!("no active runs to act on (pass a run ID)");
                    return Ok(());
                }
                1 => {
                    id_owned = active[0].id.to_string();
                    id_owned.as_str()
                }
                _ => anyhow::bail!("multiple active runs ({}); pass a run ID", active.len()),
            }
        }
    };

    let parsed: RunId = id
        .parse()
        .with_context(|| format!("invalid run ID: {id}"))?;
    let run = store
        .get_run(&parsed)?
        .ok_or_else(|| anyhow::anyhow!("run not found: {id}"))?;

    let instance_id = run
        .instance_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("run {id} has no instance recorded — cannot pull"))?;

    let handle: InstanceHandle = if let Some(instance) = store.get_instance(&instance_id)? {
        match instance.state_json.as_deref() {
            Some(json) => serde_json::from_str(json)
                .with_context(|| format!("failed to deserialize instance handle for {id}"))?,
            None => synthesize_handle(&run, &instance_id)?,
        }
    } else {
        synthesize_handle(&run, &instance_id)?
    };

    let into = match &args.into {
        Some(p) => p.clone(),
        None => runs_dir.join(run.id.to_string()).join("artifacts"),
    };
    std::fs::create_dir_all(&into)
        .with_context(|| format!("failed to create destination dir {}", into.display()))?;

    drop(store);

    let adapter_store = Store::open(db_path)?;
    let data_dir = db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let adapter = build_adapter(&run.vendor, runs_dir, config_dir, &data_dir, adapter_store)?;
    adapter.set_run_id(&run.id);

    let remote = ckpt_to_remote_pattern(&args.ckpt, args.artifacts);

    adapter
        .pull(&handle, &remote, &into)
        .with_context(|| format!("pull failed for run {id}"))?;

    report_pulled(&into, &args.ckpt)?;

    Ok(())
}

/// Map `--ckpt` selection to a remote glob hint. Kaggle's adapter ignores
/// this argument (the kernel API only exposes a "download all output" call),
/// but vast/ssh use it to scope rsync.
fn ckpt_to_remote_pattern(ckpt: &str, artifacts: bool) -> String {
    if artifacts {
        return "**/*".to_string();
    }
    match ckpt {
        "all" => "**/*".to_string(),
        "best" => "**/best*".to_string(),
        "latest" => "**/*.pt".to_string(),
        other => other.to_string(),
    }
}

/// List what landed in the destination dir, biased toward the requested
/// checkpoint flavour. Files outside the bias are still left on disk — we
/// download everything Kaggle gives us and let the user pick.
fn report_pulled(into: &Path, ckpt: &str) -> Result<()> {
    let entries: Vec<PathBuf> = walk_files(into)?;
    if entries.is_empty() {
        println!(
            "pulled to {} — no files (kernel may still be running, or output is empty)",
            into.display()
        );
        return Ok(());
    }

    println!("pulled {} file(s) to {}", entries.len(), into.display());

    let highlight: Vec<&PathBuf> = match ckpt {
        "best" => entries
            .iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.contains("best"))
            })
            .collect(),
        "latest" => {
            let mut pts: Vec<&PathBuf> = entries
                .iter()
                .filter(|p| {
                    p.extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|e| e == "pt" || e == "ckpt" || e == "safetensors")
                })
                .collect();
            pts.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
            pts.last().into_iter().copied().collect()
        }
        "all" => entries.iter().collect(),
        _ => Vec::new(),
    };

    if !highlight.is_empty() {
        println!("matching --ckpt {ckpt}:");
        for p in highlight {
            println!("  {}", p.display());
        }
    }
    Ok(())
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// Older runs (or runs whose instance row never got a `state_json`) lose the
/// serialized handle. Reconstruct just enough for `pull` to work — vendor and
/// id are all the Kaggle/vast adapters need.
fn synthesize_handle(run: &Run, instance_id: &str) -> Result<InstanceHandle> {
    Ok(InstanceHandle {
        id: instance_id.to_string(),
        vendor: run.vendor.clone(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "xrun".to_string(),
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

fn build_adapter(
    vendor: &str,
    runs_dir: &Path,
    config_dir: &Path,
    data_dir: &Path,
    store: Store,
) -> Result<Box<dyn VendorAdapter>> {
    match vendor {
        "vast" => {
            let creds = resolve_vast_credentials(config_dir);
            Ok(Box::new(VastAdapter::new(creds, store)))
        }
        "kaggle" => {
            let creds = resolve_kaggle_credentials(config_dir);
            // §1: pull's post-download ingest opens the store from store_path,
            // so hand it the data_dir (parent of runs.db). Drop our handle so
            // we don't hold an extra connection.
            drop(store);
            Ok(Box::new(
                KaggleAdapter::new()
                    .with_credentials(creds)
                    .with_store_path(data_dir.to_path_buf()),
            ))
        }
        "local" => Ok(Box::new(LocalAdapter::with_store_and_runs_dir(
            store,
            runs_dir.to_path_buf(),
        ))),
        "ssh" => {
            let creds = Credentials::load(config_dir).unwrap_or_default();
            let alias = std::env::var("XRUN_SSH_ALIAS")
                .ok()
                .or_else(|| creds.ssh_hosts.keys().next().cloned())
                .ok_or_else(|| anyhow::anyhow!("pull: no ssh hosts in credentials.toml"))?;
            let host_creds = creds
                .ssh_hosts
                .get(&alias)
                .ok_or_else(|| anyhow::anyhow!("pull: ssh alias '{alias}' missing"))?;
            let conn = SshAdapter::resolve_conn(&alias, host_creds)?;
            let workdir_root = host_creds
                .default_workdir
                .clone()
                .unwrap_or_else(|| "/tmp/xrun".to_string());
            Ok(Box::new(SshAdapter::new(store, conn, workdir_root)))
        }
        other => anyhow::bail!("pull not implemented for vendor: {other}"),
    }
}
