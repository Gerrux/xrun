#![deny(unsafe_code)]

//! `xrun gc` — reconcile vendor-side instances with the local DB.
//!
//! Two kinds of orphans exist:
//!   * **DB-orphan**: live in our DB (`destroyed_at IS NULL`) but the vendor
//!     reports them gone. Mark `destroyed_at` so they stop showing as active.
//!   * **Vendor-orphan**: running on the vendor but absent from our DB. Likely
//!     leftovers from a partial-create that never wrote `instances`. Destroyed
//!     only with `--include-unknown` to avoid clobbering instances spun up by
//!     other tooling against the same account.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use xrun_core::{
    config::credentials::VastCredentials, vendor::InstanceHandle, Credentials, Store,
    VendorAdapter,
};
use xrun_vast::VastAdapter;

use crate::cli::GcArgs;

pub fn run(args: &GcArgs, db_path: &Path, config_dir: &Path) -> Result<()> {
    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let local_active = store.list_active_instances()?;
    drop(store);

    let creds = resolve_vast_credentials(config_dir);
    let probe_store = Store::open(db_path)?;
    let probe = VastAdapter::new(creds.clone(), probe_store);
    let remote = probe.vendor_instances().with_context(|| {
        "failed to list vendor instances; check credentials with `xrun config show`"
    })?;
    let remote_ids: std::collections::HashSet<String> =
        remote.iter().map(|r| r.id.clone()).collect();
    let local_ids: std::collections::HashSet<String> =
        local_active.iter().map(|i| i.id.clone()).collect();

    let db_orphans: Vec<_> = local_active
        .iter()
        .filter(|i| !remote_ids.contains(&i.id))
        .collect();
    let vendor_orphans: Vec<_> = remote.iter().filter(|r| !local_ids.contains(&r.id)).collect();

    println!(
        "active in DB: {}   live on vendor: {}",
        local_active.len(),
        remote.len()
    );

    if db_orphans.is_empty() && vendor_orphans.is_empty() {
        println!("nothing to clean up");
        return Ok(());
    }

    if !db_orphans.is_empty() {
        println!("DB orphans (will be marked destroyed):");
        for inst in &db_orphans {
            println!("  {}  {}", inst.id, inst.gpu_type.as_deref().unwrap_or("?"));
        }
    }

    if !vendor_orphans.is_empty() {
        let action = if args.include_unknown {
            "will be destroyed"
        } else {
            "skipped (use --include-unknown to destroy)"
        };
        println!("Vendor orphans ({action}):");
        for r in &vendor_orphans {
            println!("  {}  {}", r.id, r.gpu.as_deref().unwrap_or("?"));
        }
    }

    if args.dry_run {
        println!("(dry run; no changes)");
        return Ok(());
    }

    let mut store = Store::open(db_path)?;
    let now = Utc::now();
    for inst in &db_orphans {
        store.update_instance_destroyed(&inst.id, now)?;
    }
    drop(store);

    if args.include_unknown && !vendor_orphans.is_empty() {
        for r in &vendor_orphans {
            let adapter_store = Store::open(db_path)?;
            let adapter = VastAdapter::new(creds.clone(), adapter_store);
            let handle = InstanceHandle {
                id: r.id.clone(),
                vendor: "vast".to_string(),
                ssh_host: r.ssh.as_ref().and_then(|s| s.split(':').next().map(str::to_string)),
                ssh_port: r
                    .ssh
                    .as_ref()
                    .and_then(|s| s.split(':').nth(1))
                    .and_then(|p| p.parse().ok()),
                ssh_user: "root".to_string(),
            };
            if let Err(e) = adapter.destroy(&handle) {
                eprintln!("warn: destroy {} failed: {e}", r.id);
            }
        }
    }

    println!("done");
    Ok(())
}

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
