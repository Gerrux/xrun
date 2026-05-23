#![deny(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Subcommand;
use xrun_core::{config::credentials::KaggleCredentials, paths, Credentials};
use xrun_kaggle::{snapshot, KaggleAdapter};

use crate::cli::{DatasetListArgs, DatasetPushArgs, DatasetStatusArgs, DatasetVerifyArgs};

#[derive(Subcommand)]
pub enum DatasetSubcommand {
    /// Push a local directory as a Kaggle dataset (create or new version)
    Push(DatasetPushArgs),
    /// Show the status of a Kaggle dataset
    Status(DatasetStatusArgs),
    /// List your Kaggle datasets
    List(DatasetListArgs),
    /// Smoke-check that every first-level subdir under a staging dir contains a marker file
    Verify(DatasetVerifyArgs),
}

pub fn run(subcommand: &DatasetSubcommand, config_dir: &Path) -> Result<()> {
    match subcommand {
        DatasetSubcommand::Push(args) => run_push(args, config_dir),
        DatasetSubcommand::Status(args) => run_status(args, config_dir),
        DatasetSubcommand::List(args) => run_list(args, config_dir),
        DatasetSubcommand::Verify(args) => run_verify(args),
    }
}

/// Walk first-level subdirs of `args.local_dir` and check that each one
/// contains `args.marker`. The Kaggle pre-baked-cache pitfall: a worker
/// creates `<plot>/` then crashes mid-build, leaving an empty dir that the
/// next run mistakes for "already cached". Read-only Kaggle FS turns the
/// retry into a confusing `OSError: [Errno 30]` four minutes in.
fn run_verify(args: &DatasetVerifyArgs) -> Result<()> {
    if !args.local_dir.is_dir() {
        anyhow::bail!("not a directory: {}", args.local_dir.display());
    }
    let mut missing: Vec<String> = Vec::new();
    let mut checked: u64 = 0;
    for entry in std::fs::read_dir(&args.local_dir)
        .with_context(|| format!("failed to read {}", args.local_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        checked += 1;
        let marker_path = entry.path().join(&args.marker);
        if !marker_path.exists() {
            missing.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    missing.sort();

    if args.json {
        let payload = serde_json::json!({
            "root": args.local_dir.display().to_string(),
            "marker": args.marker,
            "subdirs_checked": checked,
            "missing": missing,
            "ok": missing.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if missing.is_empty() {
        println!(
            "ok: {} subdirs under {} all contain `{}`",
            checked,
            args.local_dir.display(),
            args.marker
        );
    } else {
        println!(
            "missing `{}` in {} of {} subdirs:",
            args.marker,
            missing.len(),
            checked
        );
        for name in &missing {
            println!("  {name}");
        }
    }

    if !missing.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

fn resolve_kaggle_credentials(config_dir: &Path) -> KaggleCredentials {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.kaggle.token.is_some()
            || (creds.kaggle.username.is_some() && creds.kaggle.key.is_some())
        {
            return creds.kaggle;
        }
    }
    if let Ok(Some((username, key))) = Credentials::import_kaggle_native() {
        return KaggleCredentials {
            token: None,
            username: Some(username),
            key: Some(key),
        };
    }
    if let Ok(Some(token)) = Credentials::import_kaggle_access_token() {
        return KaggleCredentials {
            token: Some(token),
            username: None,
            key: None,
        };
    }
    KaggleCredentials::default()
}

/// Normalise a user-supplied dataset slug into the `<owner>/<name>` form Kaggle
/// requires. Token-only auth doesn't carry a username, so a user who pasted
/// only an access token has no obvious way to learn what owner string to put
/// in front of their slug — we resolve it for them, in this order:
///   1. `kaggle.username` from xrun credentials (legacy username+key auth).
///   2. `cli.authenticate()` — calls the Kaggle Python module, which knows
///      the username regardless of which auth flavour is set up.
///   3. Bail with a hint pointing at `xrun config set kaggle.username`.
///
/// A slug that already contains `/` is returned untouched (caller passed
/// `owner/name` explicitly).
fn ensure_owner_prefix(
    slug: &str,
    creds: &KaggleCredentials,
    cli: &xrun_kaggle::KaggleCli,
) -> Result<String> {
    if slug.contains('/') {
        return Ok(slug.to_string());
    }
    if let Some(user) = creds.username.as_deref() {
        if !user.is_empty() {
            return Ok(format!("{user}/{slug}"));
        }
    }
    match cli.authenticate() {
        Ok(user) => Ok(format!("{}/{}", user, slug)),
        Err(e) => anyhow::bail!(
            "dataset slug '{slug}' has no owner prefix and could not auto-resolve \
             Kaggle username: {e}\n\
             \n\
             Pass the slug as 'owner/{slug}' or set the username explicitly:\n\
             \n    xrun config set kaggle.username <your-kaggle-username>\n"
        ),
    }
}

pub fn run_push(args: &DatasetPushArgs, config_dir: &Path) -> Result<()> {
    let creds = resolve_kaggle_credentials(config_dir);
    let adapter = KaggleAdapter::new().with_credentials(creds.clone());
    let cli = adapter.cli();

    let slug = ensure_owner_prefix(&args.slug, &creds, cli)?;
    if slug != args.slug {
        eprintln!(
            "Resolved slug: {} → {}  (owner prefix added from kaggle credentials)",
            args.slug, slug
        );
    }

    let snapshots_dir = paths::data_dir().map(|d| d.join("dataset_snapshots")).ok();

    let cur_snap = snapshot::capture(&args.local_dir, &slug).with_context(|| {
        format!(
            "failed to fingerprint staging dir {}",
            args.local_dir.display()
        )
    })?;
    let prev_snap = snapshots_dir
        .as_deref()
        .and_then(|d| snapshot::load(d, &slug));
    let diff = snapshot::diff(prev_snap.as_ref(), &cur_snap);

    eprintln!(
        "Pushing {} as Kaggle dataset {}…",
        args.local_dir.display(),
        slug
    );
    if prev_snap.is_some() {
        eprintln!("Diff vs last pushed snapshot:");
    } else {
        eprintln!("No prior snapshot found — treating all files as added.");
    }
    eprintln!("{}", diff.render());
    if prev_snap.is_some() && diff.is_empty() {
        eprintln!(
            "No file changes detected. Kaggle may skip creating a new version. \
             Push will run anyway."
        );
    }

    cli.dataset_push(&args.local_dir, &slug, args.message.as_deref())
        .with_context(|| format!("failed to push dataset '{slug}'"))?;

    if let Some(dir) = snapshots_dir.as_deref() {
        if let Err(e) = snapshot::save(dir, &cur_snap) {
            tracing::warn!("could not save dataset snapshot for {slug}: {e}");
        }
    }

    if args.wait {
        eprintln!("Waiting for dataset '{slug}' to be ready…");
        let timeout = Duration::from_secs(300);
        let started = std::time::Instant::now();
        loop {
            match cli.is_dataset_ready(&slug) {
                Ok(true) => {
                    eprintln!("Dataset '{slug}' is ready.");
                    break;
                }
                Ok(false) => {
                    if started.elapsed() > timeout {
                        anyhow::bail!(
                            "dataset '{slug}' not ready after 5 minutes; \
                             check status with `xrun dataset status {slug}`"
                        );
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
                Err(e) => {
                    eprintln!("Warning: could not check dataset status: {e}");
                    break;
                }
            }
        }
    } else {
        eprintln!("Dataset push submitted. Check status with: xrun dataset status {slug}");
    }
    Ok(())
}

pub fn run_status(args: &DatasetStatusArgs, config_dir: &Path) -> Result<()> {
    let creds = resolve_kaggle_credentials(config_dir);
    let adapter = KaggleAdapter::new().with_credentials(creds.clone());
    let cli = adapter.cli();

    let slug = ensure_owner_prefix(&args.slug, &creds, cli)?;

    let raw = cli
        .dataset_status_raw(&slug)
        .with_context(|| format!("failed to get status of dataset '{slug}'"))?;

    if args.json {
        print!("{raw}");
    } else {
        // Try to pretty-print; fall back to raw
        match serde_json::from_str::<serde_json::Value>(raw.trim()) {
            Ok(v) => {
                let status = v
                    .get("status")
                    .or_else(|| v.get("datasetStatus"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                println!("{:<20}  {}", slug, status);
            }
            Err(_) => print!("{raw}"),
        }
    }
    Ok(())
}

pub fn run_list(args: &DatasetListArgs, config_dir: &Path) -> Result<()> {
    let creds = resolve_kaggle_credentials(config_dir);
    let adapter = KaggleAdapter::new().with_credentials(creds);
    let cli = adapter.cli();

    let items = cli
        .dataset_list_mine()
        .context("failed to list Kaggle datasets")?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_default()
        );
    } else {
        if items.is_empty() {
            println!("(no datasets found)");
            return Ok(());
        }
        println!("{:<40}  {:<30}  last_updated", "slug", "title");
        println!("{}", "-".repeat(90));
        for item in &items {
            println!(
                "{:<40}  {:<30}  {}",
                item.slug_ref,
                item.title.as_deref().unwrap_or("—"),
                item.last_updated.as_deref().unwrap_or("—")
            );
        }
    }
    Ok(())
}
