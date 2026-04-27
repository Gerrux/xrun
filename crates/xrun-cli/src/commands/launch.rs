#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{
    manifest::{Manifest, Vendor},
    RunStatus, Store, VendorAdapter,
};
use xrun_vast::VastStub;

use crate::cli::LaunchArgs;

pub fn run(args: &LaunchArgs, db_path: &Path, runs_dir: &Path) -> Result<()> {
    let content = std::fs::read_to_string(&args.manifest)
        .with_context(|| format!("failed to read manifest: {}", args.manifest.display()))?;

    let manifest =
        Manifest::from_yaml_str(&content).with_context(|| "manifest validation failed")?;

    let adapter: Box<dyn VendorAdapter> = match manifest.vendor {
        Vendor::Vast => Box::new(VastStub::new()),
        Vendor::Kaggle => anyhow::bail!("Kaggle adapter not implemented yet"),
    };

    adapter
        .validate(&manifest)
        .with_context(|| "vendor validation failed")?;

    if args.dry_run {
        let plan = adapter
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
        .create_run(name, &hash, &manifest_path_str, vendor_str)
        .context("failed to create run record")?;

    let run_dir = runs_dir.join(run_id.to_string());
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("failed to create run dir: {}", run_dir.display()))?;
    std::fs::copy(&args.manifest, run_dir.join("manifest.yaml"))
        .context("failed to copy manifest")?;

    eprintln!("Created run {run_id}");

    match adapter.provision(&manifest) {
        Ok(_) => anyhow::bail!("provision returned Ok unexpectedly; stub implementation bug"),
        Err(e) => {
            store
                .update_run_status(&run_id, RunStatus::Failed)
                .context("failed to update run status")?;
            anyhow::bail!("vast adapter not implemented yet: {e}");
        }
    }
}
