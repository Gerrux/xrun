#![deny(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Subcommand;
use xrun_core::{config::credentials::KaggleCredentials, Credentials};
use xrun_kaggle::KaggleAdapter;

use crate::cli::{DatasetListArgs, DatasetPushArgs, DatasetStatusArgs};

#[derive(Subcommand)]
pub enum DatasetSubcommand {
    /// Push a local directory as a Kaggle dataset (create or new version)
    Push(DatasetPushArgs),
    /// Show the status of a Kaggle dataset
    Status(DatasetStatusArgs),
    /// List your Kaggle datasets
    List(DatasetListArgs),
}

pub fn run(subcommand: &DatasetSubcommand, config_dir: &Path) -> Result<()> {
    match subcommand {
        DatasetSubcommand::Push(args) => run_push(args, config_dir),
        DatasetSubcommand::Status(args) => run_status(args, config_dir),
        DatasetSubcommand::List(args) => run_list(args, config_dir),
    }
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

pub fn run_push(args: &DatasetPushArgs, config_dir: &Path) -> Result<()> {
    let creds = resolve_kaggle_credentials(config_dir);
    let adapter = KaggleAdapter::new().with_credentials(creds);
    let cli = adapter.cli();

    eprintln!(
        "Pushing {} as Kaggle dataset {}…",
        args.local_dir.display(),
        args.slug
    );
    cli.dataset_push(&args.local_dir, &args.slug, args.message.as_deref())
        .with_context(|| format!("failed to push dataset '{}'", args.slug))?;

    if args.wait {
        eprintln!("Waiting for dataset '{}' to be ready…", args.slug);
        let timeout = Duration::from_secs(300);
        let started = std::time::Instant::now();
        loop {
            match cli.is_dataset_ready(&args.slug) {
                Ok(true) => {
                    eprintln!("Dataset '{}' is ready.", args.slug);
                    break;
                }
                Ok(false) => {
                    if started.elapsed() > timeout {
                        anyhow::bail!(
                            "dataset '{}' not ready after 5 minutes; \
                             check status with `xrun dataset status {}`",
                            args.slug,
                            args.slug
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
        eprintln!(
            "Dataset push submitted. Check status with: xrun dataset status {}",
            args.slug
        );
    }
    Ok(())
}

pub fn run_status(args: &DatasetStatusArgs, config_dir: &Path) -> Result<()> {
    let creds = resolve_kaggle_credentials(config_dir);
    let adapter = KaggleAdapter::new().with_credentials(creds);
    let cli = adapter.cli();

    let raw = cli
        .dataset_status_raw(&args.slug)
        .with_context(|| format!("failed to get status of dataset '{}'", args.slug))?;

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
                println!("{:<20}  {}", args.slug, status);
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
