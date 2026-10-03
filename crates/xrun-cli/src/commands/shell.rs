#![deny(unsafe_code)]

//! `xrun shell <id>` — drop into an interactive ssh session on the run's
//! instance. Replaces the manual `xrun show | grep ssh | copy-paste` loop
//! that otherwise starts every debug session.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use xrun_core::{vendor::InstanceHandle, Store};

use crate::cli::ShellArgs;
use crate::commands::common::{open_store, resolve_run, select_run, RunSelection};

pub fn run(args: &ShellArgs, db_path: &Path) -> Result<()> {
    let store = open_store(db_path)?;

    let handle = resolve_handle(&store, args.id.as_deref())?;
    let host = handle
        .ssh_host
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("instance has no ssh_host (still provisioning?)"))?;
    let port = handle
        .ssh_port
        .ok_or_else(|| anyhow::anyhow!("instance has no ssh_port"))?;
    let user = if handle.ssh_user.is_empty() {
        "root"
    } else {
        handle.ssh_user.as_str()
    };

    let mut cmd = Command::new("ssh");
    cmd.arg("-p")
        .arg(port.to_string())
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("ConnectTimeout=15")
        .arg(format!("{user}@{host}"));
    if let Some(remote) = &args.cmd {
        cmd.arg(remote);
    }

    let status = cmd
        .status()
        .with_context(|| "failed to spawn ssh — is it on PATH?")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn resolve_handle(store: &Store, id: Option<&str>) -> Result<InstanceHandle> {
    let instance_id = match id {
        Some(s) if s.chars().all(|c| c.is_ascii_digit()) => s.to_string(),
        Some(s) => {
            // Run id (full, prefix or tail) — resolve via the run.
            let run = resolve_run(store, s)?;
            run.instance_id
                .ok_or_else(|| anyhow::anyhow!("run {s} has no instance yet"))?
        }
        None => match select_run(store, None)? {
            RunSelection::Run(run) => run
                .instance_id
                .ok_or_else(|| anyhow::anyhow!("active run has no instance yet"))?,
            RunSelection::NoActive => anyhow::bail!("no active runs; pass an id"),
            RunSelection::Multiple(_) => {
                anyhow::bail!("multiple active runs; pass a run id or vast id")
            }
        },
    };

    let instance = store
        .get_instance(&instance_id)?
        .ok_or_else(|| anyhow::anyhow!("instance not found: {instance_id}"))?;
    let state_json = instance.state_json.ok_or_else(|| {
        anyhow::anyhow!("instance {instance_id} has no stored handle (provisioning?)")
    })?;
    let handle: InstanceHandle =
        serde_json::from_str(&state_json).context("failed to deserialize instance handle")?;
    Ok(handle)
}
