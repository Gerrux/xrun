#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{RunId, Store};

use crate::cli::RerunArgs;

pub fn run(args: &RerunArgs, db_path: &Path) -> Result<()> {
    let parsed: RunId = args
        .id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.id))?;

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    match store.get_run(&parsed).context("failed to query run")? {
        None => anyhow::bail!("run not found: {}", args.id),
        Some(_) => {
            println!("rerun not implemented yet for v0.1 (adapter pending)");
            Ok(())
        }
    }
}
