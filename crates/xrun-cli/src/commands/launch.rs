#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use xrun_core::{
    config::credentials::VastCredentials,
    manifest::{Manifest, Vendor},
    store::{RunId, RunStatus},
    Store, VendorAdapter,
};
use xrun_poller::{CancellationToken, Poller};
use xrun_vast::VastAdapter;

use crate::cli::LaunchArgs;

pub fn run(args: &LaunchArgs, db_path: &Path, runs_dir: &Path) -> Result<()> {
    let content = std::fs::read_to_string(&args.manifest)
        .with_context(|| format!("failed to read manifest: {}", args.manifest.display()))?;

    let manifest =
        Manifest::from_yaml_str(&content).with_context(|| "manifest validation failed")?;

    let vendor: Box<dyn VendorAdapter> = match manifest.vendor {
        Vendor::Vast => {
            let adapter_store = Store::open(db_path)
                .with_context(|| format!("failed to open store at {}", db_path.display()))?;
            Box::new(VastAdapter::new(VastCredentials::default(), adapter_store))
        }
        Vendor::Kaggle => anyhow::bail!("Kaggle adapter not implemented yet"),
    };

    vendor
        .validate(&manifest)
        .with_context(|| "vendor validation failed")?;

    if args.dry_run {
        let plan = vendor
            .dry_run_plan(&manifest)
            .with_context(|| "failed to compute dry-run plan")?;

        if args.json {
            let out = serde_json::json!({
                "gpu_query": plan.gpu_query,
                "estimated_price_max": plan.estimated_price_max,
                "data_total_bytes": plan.data_total_bytes,
                "cmd_line": plan.cmd_line,
                "data_items": plan.data_items.iter().map(|(src, dst)| serde_json::json!({
                    "src": src.display().to_string(),
                    "dst": dst
                })).collect::<Vec<_>>()
            });
            println!("{out}");
        } else {
            println!("DRY RUN PLAN");
            println!("  gpu_query:           {}", plan.gpu_query);
            println!("  estimated_price_max: ${:.4}/hr", plan.estimated_price_max);
            println!("  data_total_bytes:    {}", plan.data_total_bytes);
            println!("  cmd_line:            {}", plan.cmd_line);
            if !plan.data_items.is_empty() {
                println!("  data_items:");
                for (src, dst) in &plan.data_items {
                    println!("    {} -> {}", src.display(), dst);
                }
            }
        }
        return Ok(());
    }

    do_launch(args, &manifest, db_path, runs_dir, vendor)
}

/// Launch with a caller-provided vendor adapter (for testing).
pub fn run_with_vendor(
    args: &LaunchArgs,
    db_path: &Path,
    runs_dir: &Path,
    vendor: Box<dyn VendorAdapter>,
) -> Result<()> {
    let content = std::fs::read_to_string(&args.manifest)
        .with_context(|| format!("failed to read manifest: {}", args.manifest.display()))?;

    let manifest =
        Manifest::from_yaml_str(&content).with_context(|| "manifest validation failed")?;

    do_launch(args, &manifest, db_path, runs_dir, vendor)
}

fn do_launch(
    args: &LaunchArgs,
    manifest: &Manifest,
    db_path: &Path,
    runs_dir: &Path,
    vendor: Box<dyn VendorAdapter>,
) -> Result<()> {
    let hash = manifest.canonical_hash();
    let name = args.name.as_deref().unwrap_or(&manifest.name);
    let manifest_path_str = args.manifest.display().to_string();
    let vendor_str = match manifest.vendor {
        Vendor::Vast => "vast",
        Vendor::Kaggle => "kaggle",
    };

    let mut store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let run_id = store
        .create_run(
            name,
            &hash,
            &manifest_path_str,
            vendor_str,
            manifest.tags.as_deref().unwrap_or(&[]),
        )
        .context("failed to create run record")?;

    let run_dir = runs_dir.join(run_id.to_string());
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("failed to create run dir: {}", run_dir.display()))?;
    std::fs::copy(&args.manifest, run_dir.join("manifest.yaml"))
        .context("failed to copy manifest")?;

    eprintln!("Created run {run_id}");

    vendor.set_run_id(&run_id);

    // Provision
    let handle = match vendor.provision(manifest) {
        Ok(h) => h,
        Err(e) => {
            let _ = store.update_run_status(&run_id, RunStatus::Failed);
            anyhow::bail!("provision failed: {e}");
        }
    };

    // Ensure the instance row exists (VastAdapter inserts it; mock does not).
    let _ = store.insert_instance(
        &handle.id,
        &handle.vendor,
        Some(&run_id),
        None,
        None,
        Utc::now(),
    );
    let state_json =
        serde_json::to_string(&handle).context("failed to serialize instance handle")?;
    if let Err(e) = store.update_instance_state_json(&handle.id, &state_json) {
        tracing::warn!("could not persist instance handle state: {e}");
    }
    if let Err(e) = store.update_run_instance_id(&run_id, &handle.id) {
        tracing::warn!("could not link run to instance: {e}");
    }

    // Upload data sources
    let sources = manifest.data.as_deref().unwrap_or(&[]).to_vec();
    if let Err(e) = vendor.upload(&handle, &sources) {
        let _ = vendor.destroy(&handle);
        let _ = store.update_run_status(&run_id, RunStatus::Failed);
        anyhow::bail!("upload failed: {e}");
    }

    // Execute training command
    if let Err(e) = vendor.execute(&handle, &manifest.run) {
        let _ = vendor.destroy(&handle);
        let _ = store.update_run_status(&run_id, RunStatus::Failed);
        anyhow::bail!("execute failed: {e}");
    }

    // Mark as running and record start time
    let _ = store.update_run_started_at(&run_id, Utc::now());
    store
        .update_run_status(&run_id, RunStatus::Running)
        .context("failed to update run status to running")?;

    eprintln!("Run {run_id} started");

    if args.detach {
        spawn_daemon(&run_id, db_path, runs_dir)?;
        println!("{run_id}");
        return Ok(());
    }

    // Foreground poller: blocks until done/failed/cancelled
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
            eprintln!("Run {run_id} completed");
            Ok(())
        }
        Ok(RunStatus::Failed) => anyhow::bail!("run {run_id} failed"),
        Ok(RunStatus::Cancelled) => anyhow::bail!("run {run_id} was cancelled"),
        Ok(s) => anyhow::bail!("run {run_id} ended with status: {}", s.as_str()),
        Err(e) => anyhow::bail!("poller error for run {run_id}: {e}"),
    }
}

/// Spawn `xrun __poll-daemon <run_id>` as a detached background process.
pub fn spawn_daemon(run_id: &RunId, db_path: &Path, runs_dir: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("failed to determine current executable path")?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--db")
        .arg(db_path)
        .arg("__poll-daemon")
        .arg(run_id.to_string())
        .arg("--runs-dir")
        .arg(runs_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // Detach the child from the current process group / terminal.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn()
        .with_context(|| format!("failed to spawn poll-daemon for run {run_id}"))?;
    Ok(())
}
