#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use xrun_core::{RunId, Store};

use crate::cli::PullArgs;

pub fn run(args: &PullArgs, db_path: &Path) -> Result<()> {
    let id = match &args.id {
        None => {
            println!("no active runs to act on");
            return Ok(());
        }
        Some(id) => id,
    };

    let parsed: RunId = id
        .parse()
        .with_context(|| format!("invalid run ID: {id}"))?;

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    match store.get_run(&parsed).context("failed to query run")? {
        None => anyhow::bail!("run not found: {id}"),
        Some(_) => {
            println!("pull not implemented yet for v0.1 (adapter pending)");
            Ok(())
        }
    }
}
