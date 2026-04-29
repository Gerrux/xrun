#![deny(unsafe_code)]

//! `xrun rerun <id> [--patch path=value]...` — re-launch a past run, optionally
//! patching values. Loads the run's stored manifest from `runs/<id>/manifest.yaml`,
//! applies any --patch overrides, writes the new manifest to a temp file, and
//! delegates to the standard launch path.

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{manifest::Manifest, RunId, Store};

use crate::cli::{LaunchArgs, RerunArgs};
use crate::commands::{launch, patch};

pub fn run(args: &RerunArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let parsed: RunId = args
        .id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.id))?;

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;
    let run = store
        .get_run(&parsed)
        .context("failed to query run")?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", args.id))?;
    drop(store);

    // The original manifest was copied into runs/<id>/manifest.yaml at launch
    // time. That's authoritative — even if the user later edited the source
    // file, the rerun must replay the version that actually ran.
    let manifest_path = runs_dir.join(run.id.to_string()).join("manifest.yaml");
    if !manifest_path.exists() {
        anyhow::bail!(
            "manifest not found at {} — was the original run dir wiped?",
            manifest_path.display()
        );
    }
    let yaml = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest: Manifest =
        Manifest::from_yaml_str(&yaml).with_context(|| "stored manifest no longer parses")?;

    let patched = patch::apply(&manifest, &args.patch)?;

    // Write patched yaml to a temp file the launch command can read by path.
    // We pass an absolute temp path so launch's `std::path::absolute` doesn't
    // resolve to the wrong dir.
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("xrun-rerun-{}.yaml", run.id));
    let patched_yaml =
        serde_yaml::to_string(&patched).context("failed to serialize patched manifest")?;
    std::fs::write(&tmp_path, patched_yaml)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;

    let launch_args = launch_args_from_rerun(&tmp_path, &run.name);
    launch::run(&launch_args, db_path, runs_dir, config_dir)
}

fn launch_args_from_rerun(manifest_path: &Path, original_name: &str) -> LaunchArgs {
    LaunchArgs {
        manifest: manifest_path.to_path_buf(),
        dry_run: false,
        allow_duplicate: true, // the patched manifest may hash identically when args parse to the same JSON
        name: Some(format!("rerun-{original_name}")),
        json: false,
        detach: false,
        max_cost: None,
        max_hours: None,
        idle_timeout: None,
        yes: false,
        reuse_instance: None,
        upload_only: false,
        overrides: Vec::new(),
        trace: false,
    }
}
