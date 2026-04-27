#![deny(unsafe_code)]

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use std::path::Path;
use xrun_core::{
    config::{ConfigStore, Credentials, GlobalConfig},
    manifest::Vendor,
};

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Initialize config and credentials files with defaults
    Init,
    /// Show current configuration (credentials shown as <set>/<unset>)
    Show,
    /// Set a configuration value by dotted key
    Set {
        /// Config key (e.g. mlflow.url, vast.api_key)
        key: String,
        /// Value to set
        value: String,
    },
}

pub fn run(args: &ConfigArgs, config_dir: &Path) -> Result<()> {
    match &args.command {
        ConfigCommand::Init => cmd_init(config_dir),
        ConfigCommand::Show => cmd_show(config_dir),
        ConfigCommand::Set { key, value } => cmd_set(config_dir, key, value),
    }
}

fn cmd_init(config_dir: &Path) -> Result<()> {
    let result = ConfigStore::init(config_dir).context("failed to initialize config")?;
    if result.config_existed && result.creds_existed {
        println!("exists: {} (not modified)", config_dir.display());
    } else {
        println!("initialized: {}", config_dir.display());
    }
    Ok(())
}

fn cmd_show(config_dir: &Path) -> Result<()> {
    let cfg = GlobalConfig::load(config_dir)?;
    let creds = Credentials::load(config_dir)?;
    println!("# config.toml");
    print!("{}", toml::to_string_pretty(&cfg)?);
    println!("# credentials (showing which keys are set)");
    println!(
        "vast.api_key: {}",
        if creds.vast.api_key.is_some() {
            "<set>"
        } else {
            "<unset>"
        }
    );
    println!(
        "kaggle.username: {}",
        if creds.kaggle.username.is_some() {
            "<set>"
        } else {
            "<unset>"
        }
    );
    println!(
        "kaggle.key: {}",
        if creds.kaggle.key.is_some() {
            "<set>"
        } else {
            "<unset>"
        }
    );
    println!(
        "mlflow.token: {}",
        if creds.mlflow.token.is_some() {
            "<set>"
        } else {
            "<unset>"
        }
    );
    Ok(())
}

fn cmd_set(config_dir: &Path, key: &str, value: &str) -> Result<()> {
    let is_credential = matches!(
        key,
        "vast.api_key" | "kaggle.key" | "kaggle.username" | "mlflow.token"
    );

    if is_credential {
        let mut creds = Credentials::load(config_dir)?;
        match key {
            "vast.api_key" => creds.vast.api_key = Some(value.to_string()),
            "kaggle.key" => creds.kaggle.key = Some(value.to_string()),
            "kaggle.username" => creds.kaggle.username = Some(value.to_string()),
            "mlflow.token" => creds.mlflow.token = Some(value.to_string()),
            _ => {
                eprintln!("unknown config key: {key}");
                std::process::exit(64);
            }
        }
        creds.save(config_dir)?;
        println!("{key}: <set>");
    } else {
        let mut cfg = GlobalConfig::load(config_dir)?;
        match key {
            "mlflow.url" => cfg.mlflow.url = Some(value.to_string()),
            "mlflow.experiment_default" => {
                cfg.mlflow.experiment_default = Some(value.to_string());
            }
            "poller.interval_active_secs" => {
                cfg.poller.interval_active_secs =
                    value.parse().context("expected a non-negative integer")?;
            }
            "poller.interval_idle_secs" => {
                cfg.poller.interval_idle_secs =
                    value.parse().context("expected a non-negative integer")?;
            }
            "defaults.vendor" => {
                cfg.defaults.vendor = Some(match value {
                    "vast" => Vendor::Vast,
                    "kaggle" => Vendor::Kaggle,
                    _ => bail!("unknown vendor: {value}"),
                });
            }
            _ => {
                eprintln!("unknown config key: {key}");
                std::process::exit(64);
            }
        }
        cfg.save(config_dir)?;
        println!("{key} = {value}");
    }
    Ok(())
}
