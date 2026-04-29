#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{bail, Context, Result};
use xrun_core::config::credentials::Credentials;

use crate::cli::BalanceArgs;

fn resolve_api_key(config_dir: &Path) -> Option<String> {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.vast.api_key.is_some() {
            return creds.vast.api_key;
        }
    }
    Credentials::import_vast_native().ok().flatten()
}

pub fn run(args: &BalanceArgs, config_dir: &Path) -> Result<()> {
    let api_key = resolve_api_key(config_dir)
        .context("vast.api_key not set — run `xrun config init` or `xrun config set vast.api_key <key>`")?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build async runtime")?;

    let info = rt
        .block_on(xrun_vast::rest::show_user(&api_key))
        .context("failed to fetch account info from vast.ai")?;

    let balance = info.effective_balance();

    if args.json {
        let out = serde_json::json!({
            "balance_usd": balance,
        });
        println!("{out}");
    } else {
        match balance {
            Some(b) => println!("vast.ai balance: ${b:.4}"),
            None => bail!("vast.ai API did not return a balance field"),
        }
    }

    Ok(())
}
