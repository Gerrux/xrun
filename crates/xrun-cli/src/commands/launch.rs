#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use xrun_core::{
    budget,
    config::credentials::{KaggleCredentials, VastCredentials},
    manifest::{Manifest, Vendor},
    store::{InstanceCaps, RunId, RunStatus},
    vendor::InstanceHandle,
    Credentials, GlobalConfig, Store, VendorAdapter,
};
use xrun_kaggle::KaggleAdapter;
use xrun_poller::{mlflow_mirror::MlflowMirrorConfig, CancellationToken, Poller};
use xrun_vast::VastAdapter;

use crate::cli::LaunchArgs;
use crate::commands::confirm::{confirm_billable_or_exit, ConfirmEstimate};
use crate::commands::patch;

fn resolve_kaggle_credentials(config_dir: &Path) -> KaggleCredentials {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.kaggle.token.is_some()
            || (creds.kaggle.username.is_some() && creds.kaggle.key.is_some())
        {
            return creds.kaggle;
        }
    }
    // Fall back to native kaggle.json
    if let Ok(Some((username, key))) = Credentials::import_kaggle_native() {
        return KaggleCredentials {
            token: None,
            username: Some(username),
            key: Some(key),
        };
    }
    // Fall back to access_token file
    if let Ok(Some(token)) = Credentials::import_kaggle_access_token() {
        return KaggleCredentials {
            token: Some(token),
            username: None,
            key: None,
        };
    }
    KaggleCredentials::default()
}

/// Resolve `vast.api_key` from xrun's config, falling back to the legacy
/// `~/.config/vastai/vast_api_key` file. Returns `None` if neither is set —
/// callers can still proceed for `--dry-run` / `validate` paths that don't
/// touch the network.
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

pub fn run(args: &LaunchArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let content = std::fs::read_to_string(&args.manifest)
        .with_context(|| format!("failed to read manifest: {}", args.manifest.display()))?;

    let manifest =
        Manifest::from_yaml_str(&content).with_context(|| "manifest validation failed")?;
    let manifest = patch::apply(&manifest, &args.overrides)?;

    if args.trace {
        std::env::set_var("XRUN_TRACE", "1");
    }

    let global = GlobalConfig::load(config_dir).unwrap_or_default();
    let mut caps = caps_from_args_and_config(args, &global.budget);
    // Manifest policy.on_idle_minutes wins over global config, but CLI --idle-timeout
    // still beats both (already applied by caps_from_args_and_config when args.idle_timeout
    // is Some). Only apply the manifest value when the CLI flag was absent.
    if args.idle_timeout.is_none() {
        if let Some(m) = manifest.policy.as_ref().and_then(|p| p.on_idle_minutes) {
            caps.idle_timeout_secs = if m > 0 { Some(m as i64 * 60) } else { None };
        }
    }

    let vendor: Box<dyn VendorAdapter> = match manifest.vendor {
        Vendor::Vast => {
            let adapter_store = Store::open(db_path)
                .with_context(|| format!("failed to open store at {}", db_path.display()))?;
            let creds = resolve_vast_credentials(config_dir);
            let adapter = VastAdapter::with_exclude_countries(
                creds,
                adapter_store,
                global.search.exclude_countries.clone(),
            );
            adapter.set_caps(caps.clone());
            adapter.set_upload_timeout(
                manifest
                    .policy
                    .as_ref()
                    .and_then(|p| p.upload_timeout_secs)
                    .map(std::time::Duration::from_secs),
            );
            Box::new(adapter)
        }
        Vendor::Kaggle => {
            let data_dir = db_path.parent().unwrap_or(db_path);
            let kaggle_creds = resolve_kaggle_credentials(config_dir);
            Box::new(
                KaggleAdapter::new()
                    .with_store_path(data_dir.to_path_buf())
                    .with_credentials(kaggle_creds),
            )
        }
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

    // Billable confirm — uses the manifest's price cap as an upper bound on
    // hourly rate. When the manifest doesn't set a cap we still classify by
    // hourly=0 (free tier), which is intentional: an absent cap is a separate
    // problem flagged by `xrun doctor`, not by every launch.
    let plan = vendor
        .dry_run_plan(&manifest)
        .with_context(|| "failed to compute dry-run plan for confirm")?;
    let estimate = ConfirmEstimate {
        vendor: match manifest.vendor {
            Vendor::Vast => "Vast.ai".into(),
            Vendor::Kaggle => "Kaggle".into(),
        },
        gpu: plan.gpu_query.clone(),
        hourly_usd: plan.estimated_price_max,
        max_hours: caps.max_lifetime_secs.map(|s| s as f64 / 3600.0),
        max_cost_usd: caps.max_cost_usd,
        balance_usd: None,
    };
    confirm_billable_or_exit(&estimate, &global.budget, args.yes)?;

    do_launch_with_budget(
        args,
        &manifest,
        db_path,
        runs_dir,
        vendor,
        global.budget,
        global.mlflow.url.clone(),
    )
}

/// Resolve effective per-instance caps from CLI overrides + global config.
/// CLI flags win when set; otherwise inherit from `[budget]`.
fn caps_from_args_and_config(args: &LaunchArgs, cfg: &xrun_core::BudgetConfig) -> InstanceCaps {
    let mut caps = budget::caps_from_config(cfg);
    if let Some(c) = args.max_cost {
        caps.max_cost_usd = if c > 0.0 { Some(c) } else { None };
    }
    if let Some(h) = args.max_hours {
        caps.max_lifetime_secs = if h > 0.0 {
            Some((h * 3600.0) as i64)
        } else {
            None
        };
    }
    if let Some(m) = args.idle_timeout {
        caps.idle_timeout_secs = if m > 0.0 {
            Some((m * 60.0) as i64)
        } else {
            None
        };
    }
    caps
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
    do_launch_with_budget(
        args,
        manifest,
        db_path,
        runs_dir,
        vendor,
        xrun_core::BudgetConfig::default(),
        None, // no mlflow url in test path
    )
}

#[allow(clippy::too_many_arguments)]
fn do_launch_with_budget(
    args: &LaunchArgs,
    manifest: &Manifest,
    db_path: &Path,
    runs_dir: &Path,
    vendor: Box<dyn VendorAdapter>,
    budget_cfg: xrun_core::BudgetConfig,
    mlflow_url: Option<String>,
) -> Result<()> {
    let hash = manifest.canonical_hash();
    let name = args.name.as_deref().unwrap_or(&manifest.name);
    // Store an absolute path so consumers (TUI, `xrun show`) can read the
    // manifest regardless of their CWD. `path::absolute` does not resolve
    // symlinks and avoids the Windows `\\?\` UNC prefix that `canonicalize`
    // would introduce.
    let manifest_path_str = std::path::absolute(&args.manifest)
        .unwrap_or_else(|_| args.manifest.clone())
        .display()
        .to_string();
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
    // Write the (possibly patched) in-memory manifest so reruns and post-mortem
    // tooling see the values that actually ran, not the on-disk source. When
    // there were no overrides this round-trip is a no-op modulo whitespace.
    let yaml = serde_yaml::to_string(manifest)
        .unwrap_or_else(|_| std::fs::read_to_string(&args.manifest).unwrap_or_default());
    std::fs::write(run_dir.join("manifest.yaml"), yaml)
        .context("failed to write manifest copy in run dir")?;

    eprintln!("Created run {run_id}");

    vendor.set_run_id(&run_id);

    // Provision OR reuse a live instance. Reuse skips offer search +
    // create_instance entirely — the instance is already paid for, we just
    // need to reach it. The handle is reconstructed from `state_json`
    // persisted by the prior launch. New run is linked to the existing
    // instance row (one instance can serve many sequential runs).
    let (handle, reused_instance) = if let Some(reuse) = args.reuse_instance.as_deref() {
        let h = resolve_reuse_handle(&store, reuse)?;
        eprintln!(
            "Reusing instance {} ({}@{}:{})",
            h.id,
            h.ssh_user,
            h.ssh_host.as_deref().unwrap_or("?"),
            h.ssh_port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".into())
        );
        (h, true)
    } else {
        let h = match vendor.provision(manifest) {
            Ok(h) => h,
            Err(e) => {
                let _ = store.update_run_status(&run_id, RunStatus::Failed);
                anyhow::bail!("provision failed: {e}");
            }
        };
        (h, false)
    };

    // Ensure the instance row exists (VastAdapter inserts it on provision;
    // mock and reuse paths do not).
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
        // For reused instances we don't destroy on failure — the user told us
        // to keep it alive. They can `xrun stop` explicitly when done.
        if !reused_instance {
            let _ = vendor.destroy(&handle);
        }
        let _ = store.update_run_status(&run_id, RunStatus::Failed);
        anyhow::bail!("upload failed: {e}");
    }

    // --upload-only: provision + upload, then mark done and bail. The
    // instance keeps running so the user can `xrun launch --reuse-instance`
    // again or `xrun shell` in. Skip both execute and the destroy-on-error
    // path because we *want* the instance alive.
    if args.upload_only {
        store
            .update_run_status(&run_id, RunStatus::Done)
            .context("failed to mark upload-only run done")?;
        eprintln!(
            "Run {run_id} upload complete (instance {} kept alive)",
            handle.id
        );
        if args.json {
            println!(
                "{}",
                serde_json::json!({"run_id": run_id.to_string(), "instance_id": handle.id})
            );
        } else {
            println!("{run_id}");
        }
        return Ok(());
    }

    // Execute training command
    if let Err(e) = vendor.execute(&handle, &manifest.run) {
        if !reused_instance {
            let _ = vendor.destroy(&handle);
        }
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
    let poller = Poller::new(
        run_id.clone(),
        store,
        vendor,
        handle,
        runs_dir.to_path_buf(),
    )
    .with_budget(budget_cfg);

    // Wire MLflow mirroring when manifest declares an experiment and the
    // global config has an MLflow URL. Silent if either is absent.
    let poller = if let Some(cfg) = mlflow_mirror_config(manifest, mlflow_url.as_deref(), name) {
        poller.with_mlflow(cfg)
    } else {
        poller
    };

    let result = poller.run(cancel);

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

/// Resolve `--reuse-instance` to an `InstanceHandle`. Accepts either a vast
/// instance ID (numeric) or an xrun run ID (ULID); both resolve to the
/// `state_json` of the instance row in our store.
fn resolve_reuse_handle(store: &Store, id: &str) -> Result<InstanceHandle> {
    let instance_id = if id.chars().all(|c| c.is_ascii_digit()) {
        id.to_string()
    } else {
        let rid: RunId = id
            .parse()
            .with_context(|| format!("invalid --reuse-instance value: {id}"))?;
        let run = store
            .get_run(&rid)?
            .ok_or_else(|| anyhow::anyhow!("run not found: {id}"))?;
        run.instance_id
            .ok_or_else(|| anyhow::anyhow!("run {id} has no instance_id"))?
    };
    let inst = store
        .get_instance(&instance_id)?
        .ok_or_else(|| anyhow::anyhow!("instance {instance_id} not found in DB"))?;
    if inst.destroyed_at.is_some() {
        anyhow::bail!("instance {instance_id} is marked destroyed in the DB; cannot reuse");
    }
    let state_json = inst.state_json.ok_or_else(|| {
        anyhow::anyhow!("instance {instance_id} has no stored handle (mock or pre-rest run?)")
    })?;
    serde_json::from_str(&state_json).context("failed to deserialize stored instance handle")
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

/// Build a `MlflowMirrorConfig` from the manifest's `mlflow` section and the
/// global MLflow URL. Returns `None` when either is absent — poller then runs
/// without MLflow mirroring (silent degrade).
fn mlflow_mirror_config(
    manifest: &Manifest,
    mlflow_url: Option<&str>,
    run_name: &str,
) -> Option<MlflowMirrorConfig> {
    let url = mlflow_url?;
    let experiment = manifest
        .mlflow
        .as_ref()
        .and_then(|m| m.experiment.clone())?;

    Some(MlflowMirrorConfig {
        url: url.to_string(),
        experiment,
        auth: None, // token-based auth can be added via `xrun config set mlflow.token`
        log_args_as_params: manifest
            .mlflow
            .as_ref()
            .and_then(|m| m.log_args_as_params)
            .unwrap_or(true),
        run_name: Some(run_name.to_string()),
        vendor: match manifest.vendor {
            Vendor::Vast => "vast",
            Vendor::Kaggle => "kaggle",
        }
        .to_string(),
        instance_id: None,
    })
}
