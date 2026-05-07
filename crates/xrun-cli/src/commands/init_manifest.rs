//! `xrun init-manifest` — generate a manifest skeleton for a chosen
//! (vendor × sink) combination.
//!
//! Goal: Claude Code (and humans) can produce a working `xrun launch`-able
//! YAML without knowing the schema. The generator stitches a vendor block
//! and a sink block onto a shared run/artifacts skeleton, and marks every
//! field that needs human input with a `TODO_…` token so a `grep TODO_`
//! lights up everything that needs review before launch.
//!
//! Templates are static strings, not external files — keeps the binary
//! self-contained and avoids a tera/handlebars dep. Adding a vendor or
//! sink is one new constant + one match arm.

#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

#[derive(Debug, Args)]
pub struct InitManifestArgs {
    /// Vendor to target. Recognised: `vast`, `kaggle`, `local`, `ssh`.
    #[arg(long)]
    pub vendor: String,

    /// Metric sink to enable. Repeatable. Recognised: `mlflow`, `wandb`.
    /// Pass `--sink none` (or omit entirely) for a local-only mirror.
    #[arg(long = "sink", value_name = "NAME")]
    pub sinks: Vec<String>,

    /// Manifest `name:` field. Defaults to `<cwd-basename>-<vendor>`.
    #[arg(long)]
    pub name: Option<String>,

    /// Output path. Defaults to `exp/<name>.yaml`. Pass `-` to write to
    /// stdout (handy for piping into `xrun launch -`).
    #[arg(long)]
    pub into: Option<String>,

    /// Overwrite an existing file at `--into`. Without this, the command
    /// errors out so a stray rerun doesn't silently clobber edits.
    #[arg(long)]
    pub force: bool,

    /// Emit a one-line JSON summary on stdout (path, vendor, sinks).
    /// The manifest body still goes to `--into` (or stdout when `--into -`).
    #[arg(long)]
    pub json: bool,
}

/// Built-in vendors. Order matters for the `--vendor` parser only.
const VENDORS: &[&str] = &["vast", "kaggle", "local", "ssh"];
/// Built-in sinks. `none` is accepted as the "no fan-out" sentinel and
/// behaves identically to passing zero `--sink` flags.
const SINKS: &[&str] = &["mlflow", "wandb", "none"];

pub fn run(args: &InitManifestArgs) -> Result<()> {
    if !VENDORS.contains(&args.vendor.as_str()) {
        bail!(
            "unknown vendor `{}`. Recognised: {}",
            args.vendor,
            VENDORS.join(", ")
        );
    }
    let sinks: Vec<String> = args
        .sinks
        .iter()
        .filter(|s| s.as_str() != "none")
        .cloned()
        .collect();
    for s in &sinks {
        if !SINKS.contains(&s.as_str()) {
            bail!("unknown sink `{}`. Recognised: {}", s, SINKS.join(", "));
        }
    }

    let cwd_basename = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "experiment".into());
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| format!("{cwd_basename}-{}", args.vendor));

    let body = render_manifest(&name, &args.vendor, &sinks);

    let into = args.into.as_deref().unwrap_or("");
    let written_path = if into == "-" {
        // Stdout sink — used by callers that pipe directly into `xrun launch`.
        print!("{body}");
        None
    } else {
        let path: PathBuf = if into.is_empty() {
            PathBuf::from("exp").join(format!("{name}.yaml"))
        } else {
            PathBuf::from(into)
        };
        if path.exists() && !args.force {
            bail!(
                "{} already exists; pass --force to overwrite",
                path.display()
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir for {}", path.display()))?;
        }
        std::fs::write(&path, &body)
            .with_context(|| format!("writing manifest to {}", path.display()))?;
        Some(path)
    };

    if args.json {
        let path_str = written_path
            .as_deref()
            .map(Path::display)
            .map(|d| d.to_string());
        let out = serde_json::json!({
            "name": name,
            "vendor": args.vendor,
            "sinks": sinks,
            "path": path_str,
        });
        println!("{}", serde_json::to_string(&out)?);
    } else if let Some(path) = written_path {
        eprintln!("wrote {} ({} bytes)", path.display(), body.len());
        eprintln!("next: xrun doctor --manifest {}", path.display());
        eprintln!("then: xrun launch {}", path.display());
    }
    Ok(())
}

/// Stitch the manifest body. The shape is always:
///
///   header (name, description, vendor) +
///   vendor block +
///   run block +
///   artifacts block +
///   per-sink blocks +
///   trailer (notes about what to edit)
fn render_manifest(name: &str, vendor: &str, sinks: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&render_header(name, vendor, sinks));
    out.push_str(&render_vendor_block(vendor));
    out.push_str(RUN_BLOCK);
    out.push_str(ARTIFACTS_BLOCK);
    for s in sinks {
        match s.as_str() {
            "mlflow" => out.push_str(&render_mlflow_block(name)),
            "wandb" => out.push_str(WANDB_BLOCK),
            _ => {}
        }
    }
    out.push_str(&render_trailer(vendor, sinks));
    out
}

fn render_header(name: &str, vendor: &str, sinks: &[String]) -> String {
    let sinks_label = if sinks.is_empty() {
        "local-only (no remote mirror)".to_string()
    } else {
        sinks.join(" + ")
    };
    format!(
        "\
# Generated by `xrun init-manifest --vendor {vendor} {sink_flags}`.
# Edit every TODO_ token before `xrun launch`. `xrun doctor --manifest <path>`
# reports schema problems and credential gaps for the chosen vendor / sinks.
#
# vendor: {vendor}
# sinks:  {sinks_label}

name: {name}
description: |
  TODO_describe what this experiment trains and what success looks like.
vendor: {vendor}

",
        sink_flags = sinks
            .iter()
            .map(|s| format!("--sink {s}"))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn render_vendor_block(vendor: &str) -> String {
    match vendor {
        "vast" => VAST_BLOCK.to_string(),
        "kaggle" => KAGGLE_BLOCK.to_string(),
        "local" => LOCAL_BLOCK.to_string(),
        "ssh" => SSH_BLOCK.to_string(),
        _ => unreachable!("vendor pre-validated"),
    }
}

fn render_mlflow_block(name: &str) -> String {
    format!(
        "\
mlflow:
  experiment: {name}
  log_args_as_params: true   # mirror manifest run.args as MLflow params

",
    )
}

fn render_trailer(vendor: &str, sinks: &[String]) -> String {
    let mut tips: Vec<String> = Vec::new();
    if vendor == "vast" {
        tips.push(
            "# - vast: set `vast.gpu.kind` (e.g. RTX_4090) and `price.max_per_hour_usd`.".into(),
        );
    }
    if vendor == "kaggle" {
        tips.push(
            "# - kaggle: kernel_slug uses `{user}/<slug>` — owner is auto-filled from your \
             Kaggle credentials. Use a literal username if you want a fixed owner."
                .into(),
        );
        tips.push(
            "# - kaggle: `dataset` is optional; remove the line if you don't pin one.".into(),
        );
    }
    if vendor == "ssh" {
        tips.push(
            "# - ssh: configure the host alias once via `xrun config set ssh.<alias>.host …`"
                .into(),
        );
    }
    if sinks.iter().any(|s| s == "wandb") {
        tips.push(
            "# - wandb: set `xrun config set metrics.sinks \"wandb,…\"` so the poller fans out."
                .into(),
        );
    }
    if sinks.iter().any(|s| s == "mlflow") {
        tips.push(
            "# - mlflow: set `xrun config set mlflow.url …` and `xrun config set mlflow.token …`."
                .into(),
        );
    }
    if tips.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n# ── post-generation checklist ───────────────────\n");
    for t in tips {
        s.push_str(&t);
        s.push('\n');
    }
    s
}

// ── vendor blocks ───────────────────────────────────────────────────────────

const VAST_BLOCK: &str = "\
vast:
  image: pytorch/pytorch:2.2.0-cuda12.1-cudnn8-runtime
  gpu:
    kind: TODO_RTX_4090       # GPU class — `xrun balance` shows what vast offers
    count: 1
  disk_gb: 50
  price:
    max_per_hour_usd: 0.50    # offer-search cap; raise for scarce GPUs
  cuda_min: 12.1
  reliability_min: 0.95

data:
  - src: ./data/train.h5
    dst: /workspace/data/train.h5

";

const KAGGLE_BLOCK: &str = "\
kaggle:
  kernel_slug: \"{user}/TODO_kernel-slug\"  # `{user}` auto-fills your kaggle username
  enable_gpu: true                            # T4 x2 free per session
  enable_internet: false                      # Kaggle disables pip-on-the-fly
  # dataset: TODO_owner/TODO_dataset          # uncomment to pin a dataset

";

const LOCAL_BLOCK: &str = "\
local:
  gpu: cpu                  # or e.g. `cuda:0` to pin a host GPU

# `data:` block omitted — local runs read from cwd directly. Add e.g.:
#   data:
#     - src: ./data/train.h5
#       dst: ./workdir/data/train.h5
# only if your training script expects files at a different path.

";

const SSH_BLOCK: &str = "\
ssh:
  host_alias: TODO_alias    # configured via `xrun config set ssh.<alias>.host …`
  workdir: /home/TODO_user/xrun
  gpu: TODO_RTX_4090         # informational; xrun does not allocate on always-on hosts

data:
  - src: ./data/train.h5
    dst: /home/TODO_user/xrun/data/train.h5

";

const RUN_BLOCK: &str = "\
run:
  cmd: python train.py
  args:
    --epochs: 5
    --lr: 1e-4
    --batch-size: 16

";

const ARTIFACTS_BLOCK: &str = "\
artifacts:
  patterns:
    - \"checkpoints/best*.pt\"

";

const WANDB_BLOCK: &str = "\
# wandb is enabled per-host via `xrun config set metrics.sinks \"wandb,…\"`
# and `xrun config set wandb.api_key <KEY>` (or `xrun init --wandb-key -`).
# No manifest-side fields are required — the WandB sink reads its config
# from credentials.toml at launch time.

";

#[cfg(test)]
mod tests {
    use super::*;

    fn render(vendor: &str, sinks: &[&str]) -> String {
        let owned: Vec<String> = sinks.iter().map(|s| s.to_string()).collect();
        render_manifest("smoke", vendor, &owned)
    }

    #[test]
    fn vast_with_no_sinks_renders_local_only_label() {
        let s = render("vast", &[]);
        assert!(s.contains("vendor: vast"));
        assert!(s.contains("local-only (no remote mirror)"));
        assert!(s.contains("vast:"));
        assert!(!s.contains("mlflow:"));
        assert!(!s.contains("wandb"));
    }

    #[test]
    fn kaggle_with_wandb_includes_kaggle_block_and_wandb_note() {
        let s = render("kaggle", &["wandb"]);
        assert!(s.contains("kaggle:"));
        assert!(s.contains("{user}/TODO_kernel-slug"));
        assert!(s.contains("# wandb is enabled per-host via"));
    }

    #[test]
    fn vast_with_mlflow_renders_experiment_with_chosen_name() {
        let s = render("vast", &["mlflow"]);
        assert!(s.contains("mlflow:"));
        assert!(s.contains("experiment: smoke"));
        assert!(s.contains("log_args_as_params: true"));
    }

    #[test]
    fn local_skips_top_level_data_block() {
        let s = render("local", &["mlflow"]);
        assert!(s.contains("local:"));
        assert!(s.contains("gpu: cpu"));
        // Local manifests should be runnable as zero-config smokes (cf.
        // exp/templates/quickstart.yaml) — no required data: section.
        // The block we emit is a *commented* example, not a directive.
        assert!(s.contains("# `data:` block omitted"));
        assert!(
            !s.contains("\ndata:\n  - src:"),
            "active data: block must be absent for local"
        );
    }

    #[test]
    fn multiple_sinks_render_both_blocks() {
        let s = render("vast", &["mlflow", "wandb"]);
        assert!(s.contains("mlflow:"));
        assert!(s.contains("# wandb is enabled per-host via"));
    }

    #[test]
    fn header_has_grep_friendly_todo_tokens() {
        let s = render("vast", &[]);
        // `grep TODO_` should always surface at least the description and
        // GPU kind for vast — that's the contract Claude relies on.
        assert!(s.contains("TODO_describe"));
        assert!(s.contains("TODO_RTX_4090"));
    }

    #[test]
    fn ssh_trailer_warns_about_host_alias_setup() {
        let s = render("ssh", &[]);
        assert!(s.contains("# - ssh: configure the host alias once"));
    }

    #[test]
    fn wandb_trailer_warns_about_metrics_sinks_config() {
        let s = render("vast", &["wandb"]);
        assert!(s.contains("# - wandb: set `xrun config set metrics.sinks"));
    }
}
