#![deny(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Subcommand;
use xrun_core::{config::credentials::KaggleCredentials, paths};
use xrun_kaggle::{http, snapshot, KaggleAdapter};

use crate::cli::{DatasetListArgs, DatasetPushArgs, DatasetStatusArgs, DatasetVerifyArgs};
use crate::commands::common::resolve_kaggle_credentials;

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

    if !args.wait {
        eprintln!("Dataset push submitted. Check status with: xrun dataset status {slug}");
        if args.verify {
            eprintln!("Upload verification skipped (needs --wait).");
        }
        return Ok(());
    }

    eprintln!("Waiting for dataset '{slug}' to be ready…");
    let timeout = Duration::from_secs(300);
    let started = std::time::Instant::now();
    let mut ready = false;
    loop {
        match cli.is_dataset_ready(&slug) {
            Ok(true) => {
                eprintln!("Dataset '{slug}' is ready.");
                ready = true;
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

    if args.verify && ready {
        verify_upload(&creds, &slug, &cur_snap)?;
    } else if args.verify {
        eprintln!("Upload verification skipped: readiness unknown.");
    }
    Ok(())
}

/// Compare what Kaggle lists for `slug` with the staging snapshot we just
/// pushed. A mismatch is an error (exit 1): the CLI reports success and
/// `status` says `ready` even for an empty version, which is how a 917 MB
/// cache once reached a kernel as zero files. When the file list cannot be
/// fetched at all, warn and return Ok — "could not verify" is not "empty".
fn verify_upload(creds: &KaggleCredentials, slug: &str, local: &snapshot::Snapshot) -> Result<()> {
    let Some(auth) = http::auth_from_credentials(creds) else {
        eprintln!("Upload verification skipped: no Kaggle credentials for the API.");
        return Ok(());
    };
    let client = match http::KaggleApiClient::new(auth) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: could not verify upload (HTTP client): {e}");
            return Ok(());
        }
    };
    let remote = match client.dataset_files(slug) {
        Ok(files) => files,
        Err(e) => {
            eprintln!(
                "Warning: could not verify upload, Kaggle did not return the file list: {e}\n\
                 Check the dataset page by hand before launching."
            );
            return Ok(());
        }
    };

    let check = snapshot::compare_remote(local, &remote);
    if check.ok() {
        eprintln!(
            "Verified: Kaggle lists {} files ({}); all {} local files present.",
            check.remote_count,
            fmt_size(check.remote_bytes),
            check.local_count
        );
        if !check.extra.is_empty() {
            eprintln!(
                "  {} extra remote files not in staging dir (e.g. {})",
                check.extra.len(),
                preview(&check.extra)
            );
        }
        return Ok(());
    }

    eprintln!(
        "Upload mismatch: Kaggle lists {} files ({}), local staging has {} ({}).",
        check.remote_count,
        fmt_size(check.remote_bytes),
        check.local_count,
        fmt_size(check.local_bytes)
    );
    eprintln!(
        "  missing on Kaggle: {} files, e.g. {}",
        check.missing.len(),
        preview(&check.missing)
    );
    if !check.extra.is_empty() {
        eprintln!(
            "  present on Kaggle but not local: {} files, e.g. {}",
            check.extra.len(),
            preview(&check.extra)
        );
    }
    let archive_hint = check
        .extra
        .iter()
        .any(|n| n.ends_with(".tar") || n.ends_with(".zip"));
    if archive_hint {
        eprintln!(
            "  Kaggle kept the subdirectory archives instead of extracting them; \
             a kernel will see `train.tar`, not `train/`."
        );
    }
    anyhow::bail!(
        "dataset '{slug}' on Kaggle does not match the staging dir; \
         do not launch against it (pass --verify=false to override)"
    )
}

fn preview(paths: &[String]) -> String {
    const N: usize = 5;
    let shown: Vec<&str> = paths.iter().take(N).map(String::as_str).collect();
    if paths.len() > N {
        format!("{} … (+{})", shown.join(", "), paths.len() - N)
    } else {
        shown.join(", ")
    }
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
        return Ok(());
    }

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
        Err(_) => println!("{}", raw.trim_end()),
    }

    // `ready` alone says nothing about content (an empty version is `ready`
    // too). Add what Kaggle actually lists; a failed lookup is reported as
    // such rather than hidden, but never fails the status command.
    if let Some(auth) = http::auth_from_credentials(&creds) {
        if let Ok(client) = http::KaggleApiClient::new(auth) {
            match client.dataset_files(&slug) {
                Ok(files) => {
                    let known: Vec<u64> = files.iter().filter_map(|f| f.total_bytes).collect();
                    let size = if known.is_empty() {
                        "n/a".to_string()
                    } else {
                        fmt_size(known.iter().sum())
                    };
                    println!("files: {}  size: {}", files.len(), size);
                }
                Err(e) => println!("files: n/a  (Kaggle did not return the file list: {e})"),
            }
        }
    }
    Ok(())
}

/// Human size: KiB below 1 MiB, otherwise MiB with one decimal.
fn fmt_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
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
