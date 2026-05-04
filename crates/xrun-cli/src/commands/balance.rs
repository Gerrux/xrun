#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::config::credentials::Credentials;

use crate::cli::BalanceArgs;

fn resolve_vast_key(config_dir: &Path) -> Option<String> {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.vast.api_key.is_some() {
            return creds.vast.api_key;
        }
    }
    Credentials::import_vast_native().ok().flatten()
}

/// Best-effort Kaggle GPU/TPU quota readout. The Kaggle public API exposes
/// no quota endpoint; users can paste their remaining hours from the Kaggle
/// settings page (https://www.kaggle.com/settings) into the env var below.
/// If unset we return `None` and `xrun balance` falls back to the static
/// "30h/week" message.
fn kaggle_quota_remaining_hours() -> Option<f64> {
    std::env::var("KAGGLE_GPU_QUOTA_REMAINING_HOURS")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|h| (0.0..=200.0).contains(h))
}

fn kaggle_configured(config_dir: &Path) -> bool {
    // Check env var / access_token file first (no disk config needed).
    if Credentials::import_kaggle_access_token()
        .ok()
        .flatten()
        .is_some()
    {
        return true;
    }
    Credentials::load(config_dir)
        .map(|c| {
            c.kaggle.token.is_some() || (c.kaggle.username.is_some() && c.kaggle.key.is_some())
        })
        .unwrap_or(false)
}

pub fn run(args: &BalanceArgs, config_dir: &Path) -> Result<()> {
    let vast_key = resolve_vast_key(config_dir);
    let kaggle = kaggle_configured(config_dir);

    if !args.json && vast_key.is_none() && !kaggle {
        anyhow::bail!(
            "no vendor credentials configured\n\
             Run `xrun config init` or set credentials in the TUI (xrun → V)"
        );
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build async runtime")?;

    if args.json {
        let mut obj = serde_json::json!({});

        if let Some(key) = vast_key {
            match rt.block_on(xrun_vast::rest::show_user(&key)) {
                Ok(info) => {
                    obj["vast"] = serde_json::json!({
                        "balance_usd": info.effective_balance(),
                        "ok": true,
                    });
                }
                Err(e) => {
                    obj["vast"] = serde_json::json!({ "ok": false, "error": e.to_string() });
                }
            }
        }

        if kaggle {
            let mut k = serde_json::json!({
                "ok": true,
                "weekly_quota_hours": 30,
                "note": "Kaggle has no public quota endpoint. Set KAGGLE_GPU_QUOTA_REMAINING_HOURS to override.",
            });
            if let Some(h) = kaggle_quota_remaining_hours() {
                k["remaining_hours"] = serde_json::json!(h);
                k["source"] = serde_json::json!("env:KAGGLE_GPU_QUOTA_REMAINING_HOURS");
            }
            obj["kaggle"] = k;
        }

        println!("{obj}");
    } else {
        if let Some(key) = vast_key {
            match rt.block_on(xrun_vast::rest::show_user(&key)) {
                Ok(info) => match info.effective_balance() {
                    Some(b) => println!("vast.ai    ${b:.4}"),
                    None => println!("vast.ai    (balance unavailable)"),
                },
                Err(e) => println!("vast.ai    error: {e}"),
            }
        }

        if kaggle {
            match kaggle_quota_remaining_hours() {
                Some(h) => println!(
                    "kaggle     {h:.1}h GPU remaining (from KAGGLE_GPU_QUOTA_REMAINING_HOURS) of 30h/week cap"
                ),
                None => println!(
                    "kaggle     free tier — GPU quota 30h/week (set KAGGLE_GPU_QUOTA_REMAINING_HOURS to track remainder)"
                ),
            }
        }
    }

    Ok(())
}
