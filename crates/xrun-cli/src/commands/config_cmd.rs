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
                    "mlflow.username": creds.mlflow.username.is_some(),
                    "mlflow.password": creds.mlflow.password.is_some(),
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
                        "mlflow.username": creds.mlflow.username.as_deref().map(tail6),
                        "mlflow.password": creds.mlflow.password.as_deref().map(tail6),
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
    print_cred("mlflow.username", creds.mlflow.username.as_deref(), secrets);
    print_cred("mlflow.password", creds.mlflow.password.as_deref(), secrets);
    Ok(())
}

fn print_cred(key: &str, value: Option<&str>, secrets: bool) {
    match (value, secrets) {
        (Some(v), true) => println!("{key}: <set ...{}>", tail6(v)),
        (Some(_), false) => println!("{key}: <set>"),
        (None, _) => println!("{key}: <unset>"),
    }
}

/// Set an SSH host field: `<alias>.<field>` where field ∈
/// {host, user, port, key, default_workdir}. Aliases must be alphanumeric
/// (plus `-` / `_`) so they round-trip through TOML keys without quoting.
fn cmd_set_ssh(config_dir: &Path, rest: &str, value: &str) -> Result<()> {
    let (alias, field) = rest.split_once('.').ok_or_else(|| {
        anyhow::anyhow!(
            "SSH key must be `ssh.<alias>.<field>` (field: host/user/port/key/default_workdir)"
        )
    })?;
    if alias.is_empty() || !alias.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        bail!("invalid SSH alias `{alias}`: must be alphanumeric (plus - or _)");
    }

    let mut creds = Credentials::load(config_dir)?;
    let entry = creds.ssh_hosts.entry(alias.to_string()).or_default();
    match field {
        "host" => entry.host = Some(value.to_string()),
        "user" => entry.user = Some(value.to_string()),
        "port" => {
            entry.port = Some(
                value
                    .parse::<u16>()
                    .context("ssh.<alias>.port expected an integer 0..=65535")?,
            )
        }
        "key" => entry.key = Some(value.to_string()),
        "default_workdir" => entry.default_workdir = Some(value.to_string()),
        other => bail!(
            "unknown SSH field `{other}` (expected host/user/port/key/default_workdir)"
        ),
    }
    creds.save(config_dir)?;
    println!("ssh.{alias}.{field}: <set>");
    Ok(())
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
    // SSH host: `ssh.<alias>.<field>` where field ∈ {host, user, port, key, default_workdir}.
    // Stored in credentials.toml under `[ssh.<alias>]`.
    if let Some(rest) = key.strip_prefix("ssh.") {
        return cmd_set_ssh(config_dir, rest, value);
    }

    let is_credential = matches!(
        key,
        "vast.api_key"
            | "kaggle.token"
            | "kaggle.key"
            | "kaggle.username"
            | "mlflow.token"
            | "mlflow.username"
            | "mlflow.password"
    );

    if is_credential {
        let mut creds = Credentials::load(config_dir)?;
        match key {
            "vast.api_key" => creds.vast.api_key = Some(value.to_string()),
            "kaggle.token" => creds.kaggle.token = Some(value.to_string()),
            "kaggle.key" => creds.kaggle.key = Some(value.to_string()),
            "kaggle.username" => creds.kaggle.username = Some(value.to_string()),
            "mlflow.token" => creds.mlflow.token = Some(value.to_string()),
            "mlflow.username" => creds.mlflow.username = Some(value.to_string()),
            "mlflow.password" => creds.mlflow.password = Some(value.to_string()),
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
            "ui.wizard_completed" => {
                cfg.ui.wizard_completed = match value {
                    "true" | "1" | "yes" => true,
                    "false" | "0" | "no" => false,
                    _ => bail!("expected boolean (true/false), got: {value}"),
                };
            }
            "metrics.sinks" => {
                cfg.metrics.sinks = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
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
