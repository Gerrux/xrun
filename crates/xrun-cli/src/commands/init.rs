//! `xrun init` — first-run wizard entry point.
//!
//! Three modes share the command:
//!
//! * **Interactive** (default on a TTY, no flags): spawn `xrun-tui --wizard`.
//!   Returns the TUI's exit code.
//! * **Non-interactive** (`--non-interactive`): write a partial config from
//!   flags. Used by CI, the Claude skill, and `--probe-local` callers.
//! * **Probe-only** (`--probe-local --json`): emit JSON describing the local
//!   machine's capabilities (GPUs, OS) and exit. Pure read; no config writes.
//!
//! `--mark-completed` flips `[ui] wizard_completed = true`. The TUI calls this
//! after the user finishes (or skips) the wizard so we don't re-prompt.
//!
//! Credential flags (`--vast-key`, `--kaggle-token`, `--kaggle-username`,
//! `--kaggle-key`) accept a literal value or `-` to read one trimmed line from
//! stdin. Only one `-` is allowed per invocation. They require `--non-interactive`.

#![deny(unsafe_code)]

use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Args;
use serde_json::json;
use xrun_core::config::{ConfigStore, Credentials, GlobalConfig};

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Skip the TUI wizard and write config from flags only. Required when
    /// stdout is not a TTY (CI / scripted use).
    #[arg(long)]
    pub non_interactive: bool,

    /// Probe local capabilities (GPU, OS) and emit them. Pure read — no
    /// config writes. Pair with `--json` for machine-readable output.
    #[arg(long)]
    pub probe_local: bool,

    /// Metric mirror sink to enable. Repeatable. Recognised: `mlflow`.
    /// `wandb` and `comet` arrive in v0.8.
    #[arg(long = "sink", value_name = "NAME")]
    pub sinks: Vec<String>,

    /// Mark the wizard as completed (sets `[ui] wizard_completed = true`).
    /// The TUI calls this after the user finishes or skips the wizard.
    #[arg(long)]
    pub mark_completed: bool,

    /// Vast.ai API key. Pass `-` to read from stdin. Requires --non-interactive.
    /// Note: literal values appear in shell history — prefer `-` with stdin
    /// piping, or set via `xrun config set vast.api_key`.
    #[arg(long, value_name = "KEY")]
    pub vast_key: Option<String>,

    /// Kaggle JWT access token (preferred). Pass `-` to read from stdin.
    /// Requires --non-interactive.
    #[arg(long, value_name = "TOKEN")]
    pub kaggle_token: Option<String>,

    /// Kaggle username (legacy username+key auth — pair with --kaggle-key).
    /// Requires --non-interactive.
    #[arg(long, value_name = "USERNAME")]
    pub kaggle_username: Option<String>,

    /// Kaggle API key (legacy username+key auth — pair with --kaggle-username).
    /// Pass `-` to read from stdin. Requires --non-interactive.
    #[arg(long, value_name = "KEY")]
    pub kaggle_key: Option<String>,

    /// Emit machine-readable JSON. Affects `--probe-local` and the summary
    /// printed at the end.
    #[arg(long)]
    pub json: bool,

    /// After writing credentials, probe each configured vendor (vast `show user`,
    /// kaggle `KaggleApi.authenticate()`). On success implicitly sets
    /// `wizard_completed=true` (no extra `--mark-completed` needed). On
    /// failure exit non-zero and leave the flag untouched. Pairs with
    /// `--non-interactive`.
    #[arg(long)]
    pub validate_creds: bool,
}

pub fn run(args: &InitArgs, config_dir: &Path) -> Result<()> {
    if args.probe_local {
        return probe_local(args.json);
    }

    // Make sure config files exist before we touch them.
    ConfigStore::init(config_dir).context("failed to initialize config")?;

    if args.non_interactive {
        return non_interactive(args, config_dir);
    }

    if has_credential_flags(args) {
        bail!("credential flags require --non-interactive");
    }

    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        bail!(
            "xrun init requires a TTY for interactive mode. \
             Use --non-interactive (with --sink / --mark-completed / --vast-key / --kaggle-token) for scripted setup, \
             or --probe-local --json for capability detection."
        );
    }

    spawn_wizard_tui()
}

fn has_credential_flags(args: &InitArgs) -> bool {
    args.vast_key.is_some()
        || args.kaggle_token.is_some()
        || args.kaggle_username.is_some()
        || args.kaggle_key.is_some()
}

fn spawn_wizard_tui() -> Result<()> {
    let status = Command::new("xrun-tui")
        .arg("--wizard")
        .status()
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to launch xrun-tui: {e}\n\
                 Install with: pip install -e python/xrun_tui"
            )
        })?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Read one trimmed line from stdin. Used when a credential flag is `-`.
/// Stdin can only be consumed once per process, so we cache the result.
fn read_stdin_once(consumed: &mut bool) -> Result<String> {
    if *consumed {
        bail!("only one credential flag can be set to `-` per invocation");
    }
    *consumed = true;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("failed to read stdin for credential")?;
    let line = buf.trim();
    if line.is_empty() {
        bail!("stdin was empty — expected credential value");
    }
    // If multiline was piped, take the first line and warn nothing —
    // most secrets are one line and a trailing newline is common.
    Ok(line.lines().next().unwrap_or(line).trim().to_string())
}

fn resolve_credential(value: &Option<String>, stdin_used: &mut bool) -> Result<Option<String>> {
    match value {
        None => Ok(None),
        Some(s) if s == "-" => Ok(Some(read_stdin_once(stdin_used)?)),
        Some(s) => Ok(Some(s.clone())),
    }
}

fn non_interactive(args: &InitArgs, config_dir: &Path) -> Result<()> {
    let mut cfg = GlobalConfig::load(config_dir)?;
    let mut config_changed = false;

    let valid_sinks = ["mlflow"];
    for s in &args.sinks {
        if !valid_sinks.contains(&s.as_str()) {
            bail!(
                "unknown sink: {s}. Recognised: {} (wandb/comet arrive in v0.8)",
                valid_sinks.join(", ")
            );
        }
    }
    if !args.sinks.is_empty() {
        cfg.metrics.sinks = args.sinks.clone();
        config_changed = true;
    }

    if args.mark_completed && !cfg.ui.wizard_completed {
        cfg.ui.wizard_completed = true;
        config_changed = true;
    }

    if config_changed {
        cfg.save(config_dir)?;
    }

    // Credentials: resolve all four (one stdin read max), then save once.
    let mut stdin_used = false;
    let vast_key = resolve_credential(&args.vast_key, &mut stdin_used)?;
    let kaggle_token = resolve_credential(&args.kaggle_token, &mut stdin_used)?;
    let kaggle_username = resolve_credential(&args.kaggle_username, &mut stdin_used)?;
    let kaggle_key = resolve_credential(&args.kaggle_key, &mut stdin_used)?;

    // Sanity: kaggle_username and kaggle_key are a pair (legacy auth).
    match (&kaggle_username, &kaggle_key) {
        (Some(_), None) => bail!("--kaggle-username requires --kaggle-key"),
        (None, Some(_)) => bail!(
            "--kaggle-key (legacy) requires --kaggle-username; \
                                 use --kaggle-token for token auth"
        ),
        _ => {}
    }

    let mut creds_changed = false;
    let mut creds_set: Vec<&str> = Vec::new();
    if vast_key.is_some() || kaggle_token.is_some() || kaggle_username.is_some() {
        let mut creds = Credentials::load(config_dir)?;
        if let Some(k) = vast_key {
            creds.vast.api_key = Some(k);
            creds_set.push("vast.api_key");
            creds_changed = true;
        }
        if let Some(t) = kaggle_token {
            creds.kaggle.token = Some(t);
            creds_set.push("kaggle.token");
            creds_changed = true;
        }
        if let Some(u) = kaggle_username {
            creds.kaggle.username = Some(u);
            creds_set.push("kaggle.username");
            creds_changed = true;
        }
        if let Some(k) = kaggle_key {
            creds.kaggle.key = Some(k);
            creds_set.push("kaggle.key");
            creds_changed = true;
        }
        if creds_changed {
            creds.save(config_dir)?;
        }
    }

    let mut probe_results: Vec<(String, Result<String, String>)> = Vec::new();
    if args.validate_creds {
        let creds = Credentials::load(config_dir)?;
        if creds.vast.api_key.is_some() {
            let key = creds.vast.api_key.clone().unwrap();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to build async runtime for vast probe")?;
            let res = match rt.block_on(xrun_vast::rest::show_user(&key)) {
                Ok(info) => Ok(info
                    .effective_balance()
                    .map(|b| format!("balance ${b:.2}"))
                    .unwrap_or_else(|| "ok".into())),
                Err(e) => Err(e.to_string()),
            };
            probe_results.push(("vast".into(), res));
        }
        let kaggle_configured = creds.kaggle.token.is_some()
            || (creds.kaggle.username.is_some() && creds.kaggle.key.is_some());
        if kaggle_configured {
            let adapter = xrun_kaggle::KaggleAdapter::new().with_credentials(creds.kaggle.clone());
            let res = adapter
                .cli()
                .authenticate()
                .map(|u| format!("authenticated as {u}"))
                .map_err(|e| e.to_string());
            probe_results.push(("kaggle".into(), res));
        }
        let any_failed = probe_results.iter().any(|(_, r)| r.is_err());
        if !any_failed && !probe_results.is_empty() && !cfg.ui.wizard_completed {
            cfg.ui.wizard_completed = true;
            cfg.save(config_dir)?;
            config_changed = true;
        }
        if any_failed {
            for (vendor, r) in &probe_results {
                if let Err(e) = r {
                    eprintln!("vendor probe failed: {vendor}: {e}");
                }
            }
            anyhow::bail!(
                "credential validation failed — wizard_completed left unset. \
                 Re-run `xrun init --non-interactive --validate-creds` after fixing creds."
            );
        }
    }

    let changed = config_changed || creds_changed;

    let probes_json: serde_json::Value = if probe_results.is_empty() {
        json!(null)
    } else {
        serde_json::Value::Object(
            probe_results
                .iter()
                .map(|(name, r)| {
                    (
                        name.clone(),
                        match r {
                            Ok(d) => json!({"ok": true, "detail": d}),
                            Err(e) => json!({"ok": false, "error": e}),
                        },
                    )
                })
                .collect(),
        )
    };
    let summary = json!({
        "config_dir": config_dir.display().to_string(),
        "wizard_completed": cfg.ui.wizard_completed,
        "metrics_sinks": cfg.metrics.sinks,
        "credentials_set": creds_set,
        "changed": changed,
        "probes": probes_json,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "wizard_completed = {}\nmetrics.sinks = [{}]\ncredentials set: [{}]\n{}",
            cfg.ui.wizard_completed,
            cfg.metrics.sinks.join(", "),
            creds_set.join(", "),
            if changed {
                "config updated"
            } else {
                "no changes"
            }
        );
    }
    Ok(())
}

fn probe_local(json_out: bool) -> Result<()> {
    let gpus = xrun_local::process::probe_gpu_summary();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    if json_out {
        let value = json!({
            "os": os,
            "arch": arch,
            "gpus": gpus,
            "gpu_available": !gpus.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("os: {os} ({arch})");
        if gpus.is_empty() {
            println!("gpu: none detected (CPU-only — fine for smoke tests)");
        } else {
            for g in &gpus {
                println!("gpu: {g}");
            }
        }
    }
    Ok(())
}
