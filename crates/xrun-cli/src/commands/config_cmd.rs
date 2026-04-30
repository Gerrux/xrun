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
    Show {
        /// Emit machine-readable JSON instead of TOML.
        #[arg(long)]
        json: bool,
        /// Reveal the last 6 characters of each set credential (helps confirm
        /// you are using the key you think you are without printing the whole
        /// thing).
        #[arg(long)]
        secrets: bool,
    },
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
        ConfigCommand::Show { json, secrets } => cmd_show(config_dir, *json, *secrets),
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

fn cmd_show(config_dir: &Path, json: bool, secrets: bool) -> Result<()> {
    let cfg = GlobalConfig::load(config_dir)?;
    let creds = Credentials::load(config_dir)?;
    if json {
        let mut value = serde_json::to_value(&cfg)?;
        if let serde_json::Value::Object(ref mut map) = value {
            map.insert(
                "_credentials_set".into(),
                serde_json::json!({
                    "vast.api_key": creds.vast.api_key.is_some(),
                    "kaggle.token": creds.kaggle.token.is_some(),
                    "kaggle.username": creds.kaggle.username.is_some(),
                    "kaggle.key": creds.kaggle.key.is_some(),
                    "mlflow.token": creds.mlflow.token.is_some(),
                }),
            );
            if secrets {
                map.insert(
                    "_credentials_tail".into(),
                    serde_json::json!({
                        "vast.api_key": creds.vast.api_key.as_deref().map(tail6),
                        "kaggle.token": creds.kaggle.token.as_deref().map(tail6),
                        "kaggle.username": creds.kaggle.username.as_deref().map(tail6),
                        "kaggle.key": creds.kaggle.key.as_deref().map(tail6),
                        "mlflow.token": creds.mlflow.token.as_deref().map(tail6),
                    }),
                );
            }
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("# config.toml");
    print!("{}", toml::to_string_pretty(&cfg)?);
    println!("# credentials");
    print_cred("vast.api_key", creds.vast.api_key.as_deref(), secrets);
    print_cred("kaggle.token", creds.kaggle.token.as_deref(), secrets);
    print_cred("kaggle.username", creds.kaggle.username.as_deref(), secrets);
    print_cred("kaggle.key", creds.kaggle.key.as_deref(), secrets);
    print_cred("mlflow.token", creds.mlflow.token.as_deref(), secrets);
    Ok(())
}

fn print_cred(key: &str, value: Option<&str>, secrets: bool) {
    match (value, secrets) {
        (Some(v), true) => println!("{key}: <set ...{}>", tail6(v)),
        (Some(_), false) => println!("{key}: <set>"),
        (None, _) => println!("{key}: <unset>"),
    }
}

/// Last 6 characters of a credential. Used to confirm you're shipping the key
/// you think you are without leaking the whole secret to the terminal.
fn tail6(s: &str) -> String {
    let n = s.chars().count();
    if n <= 6 {
        "*".repeat(n)
    } else {
        s.chars().skip(n - 6).collect()
    }
}

fn cmd_set(config_dir: &Path, key: &str, value: &str) -> Result<()> {
    let is_credential = matches!(
        key,
        "vast.api_key" | "kaggle.token" | "kaggle.key" | "kaggle.username" | "mlflow.token"
    );

    if is_credential {
        let mut creds = Credentials::load(config_dir)?;
        match key {
            "vast.api_key" => creds.vast.api_key = Some(value.to_string()),
            "kaggle.token" => creds.kaggle.token = Some(value.to_string()),
            "kaggle.key" => creds.kaggle.key = Some(value.to_string()),
            "kaggle.username" => creds.kaggle.username = Some(value.to_string()),
            "mlflow.token" => creds.mlflow.token = Some(value.to_string()),
            _ => {
                bail!("unknown config key: {key}");
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
            "search.exclude_countries" => {
                cfg.search.exclude_countries = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "defaults.vendor" => {
                cfg.defaults.vendor = Some(match value {
                    "vast" => Vendor::Vast,
                    "kaggle" => Vendor::Kaggle,
                    _ => bail!("unknown vendor: {value}"),
                });
            }
            _ => {
                bail!("unknown config key: {key}");
            }
        }
        cfg.save(config_dir)?;
        println!("{key} = {value}");
    }
    Ok(())
}
