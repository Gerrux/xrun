#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use xrun_core::{store::Run, vendor::InstanceHandle};

use crate::cli::PullArgs;
use crate::commands::common::{
    build_adapter, open_store, select_run, AdapterCtx, RunSelection, SshSource,
};

pub fn run(args: &PullArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let store = open_store(db_path)?;

    let run = match select_run(&store, args.id.as_deref())? {
        RunSelection::Run(run) => *run,
        RunSelection::NoActive => {
            println!("no active runs to act on (pass a run ID)");
            return Ok(());
        }
        RunSelection::Multiple(n) => {
            anyhow::bail!("multiple active runs ({n}); pass a run ID")
        }
    };
    // Full id for messages: the user may have typed an abbreviation.
    let id = run.id.to_string();
    let id = id.as_str();

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

    // XRUN_SSH_ALIAS overrides; otherwise the host the run was launched on.
    // Unlike stop, pull still works when the saved manifest copy is gone.
    let mut ctx = AdapterCtx::new("pull", db_path, runs_dir, config_dir, &run.id);
    ctx.ssh = SshSource::EnvThenSavedManifest;
    let adapter = build_adapter(&run.vendor, &ctx)?;

    let mut remote = ckpt_to_remote_pattern(&args.ckpt, args.artifacts);
    if run.vendor == "vast" {
        // vast globs from $HOME; training ran in the manifest's workdir.
        let workdir = saved_run_workdir(&run_dir_of(runs_dir, &run));
        remote = xrun_core::manifest::anchor_vast_pattern(workdir.as_deref(), &remote);
    } else if run.vendor == "ssh" {
        // ssh training ran in `run.workdir` when set; without it the adapter
        // anchors relative patterns at the per-run dir itself.
        let workdir = saved_run_workdir(&run_dir_of(runs_dir, &run));
        if let Some(dir) = xrun_core::manifest::ssh_workdir_anchor(workdir.as_deref()) {
            remote = xrun_core::manifest::anchor_vast_pattern(Some(&dir), &remote);
        }
    }

    adapter
        .pull(&handle, &remote, &into)
        .with_context(|| format!("pull failed for run {id}"))?;

    report_pulled(&into, &args.ckpt)?;

    Ok(())
}

// Kaggle's adapter ignores the pattern (the kernel API only exposes a
// "download all output" call), but vast/ssh use it to scope the transfer.
use xrun_core::manifest::ckpt_to_remote_pattern;

fn run_dir_of(runs_dir: &Path, run: &Run) -> PathBuf {
    runs_dir.join(run.id.to_string())
}

/// `run.workdir` from the run's saved manifest copy (`None` = default).
fn saved_run_workdir(run_dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(run_dir.join("manifest.yaml")).ok()?;
    let manifest: xrun_core::manifest::Manifest = serde_yaml::from_str(&content).ok()?;
    manifest.run.workdir
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
        run_dir: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_workdir_is_read_from_manifest_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(saved_run_workdir(tmp.path()), None);
        std::fs::write(
            tmp.path().join("manifest.yaml"),
            "name: x\nvendor: local\nrun:\n  cmd: python t.py\n  workdir: /srv/app\n",
        )
        .unwrap();
        assert_eq!(saved_run_workdir(tmp.path()).as_deref(), Some("/srv/app"));
    }
}
