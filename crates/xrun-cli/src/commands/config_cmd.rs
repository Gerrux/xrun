#![deny(unsafe_code)]

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use serde_json::Value;
use std::path::Path;
use std::str::FromStr;
use xrun_core::{
    config::{ConfigStore, Credentials, GlobalConfig, VendorDefaults},
    manifest::Vendor,
};

use crate::commands::probe::ProbeArgs;

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
    /// Probe a vendor / sink with the credentials in `XRUN_PROBE_*` env vars.
    /// Used by the first-run wizard to validate pasted keys before persisting
    /// them. Output is one JSON object on stdout; exit code is always 0.
    Probe(ProbeArgs),
}

pub fn run(args: &ConfigArgs, config_dir: &Path) -> Result<()> {
    match &args.command {
        ConfigCommand::Init => cmd_init(config_dir),
        ConfigCommand::Show { json, secrets } => cmd_show(config_dir, *json, *secrets),
        ConfigCommand::Set { key, value } => cmd_set(config_dir, key, value),
        ConfigCommand::Probe(args) => crate::commands::probe::run(args),
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
    if alias.is_empty()
        || !alias
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
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
        other => bail!("unknown SSH field `{other}` (expected host/user/port/key/default_workdir)"),
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
    // SSH host: `ssh.<alias>.<field>`. Aliases are arbitrary user strings, so
    // they need a custom path-rule and can't go through the schema-driven
    // setter below (which expects every path segment to exist in the struct).
    if let Some(rest) = key.strip_prefix("ssh.") {
        return cmd_set_ssh(config_dir, rest, value);
    }

    // Per-vendor defaults: `vendors.<name>.<field>` lives in a HashMap that
    // is empty by default, so the entry has to be auto-created. Validating
    // <name> against the Vendor registry catches typos before they get
    // serialized.
    if let Some(rest) = key.strip_prefix("vendors.") {
        return cmd_set_vendor(config_dir, rest, value);
    }

    if is_credential_key(key) {
        let creds = Credentials::load(config_dir)?;
        let mut json = serde_json::to_value(&creds)?;
        let defaults = serde_json::to_value(Credentials::default())?;
        set_dotted(&mut json, key, value, Some(&defaults))?;
        let updated: Credentials =
            serde_json::from_value(json).with_context(|| format!("invalid value for `{key}`"))?;
        updated.save(config_dir)?;
        println!("{key}: <set>");
        return Ok(());
    }

    // `defaults.vendor` is a string on the JSON wire but only a closed set is
    // valid. Validate up-front so users see the accepted vendors instead of a
    // serde error.
    if key == "defaults.vendor" {
        Vendor::from_str(value).map_err(|e| anyhow!(e))?;
    }

    let cfg = GlobalConfig::load(config_dir)?;
    let mut json = serde_json::to_value(&cfg)?;
    let defaults = serde_json::to_value(GlobalConfig::default())?;
    set_dotted(&mut json, key, value, Some(&defaults))?;
    let updated: GlobalConfig =
        serde_json::from_value(json).with_context(|| format!("invalid value for `{key}`"))?;
    updated.save(config_dir)?;
    println!("{key} = {value}");
    Ok(())
}

fn is_credential_key(k: &str) -> bool {
    matches!(
        k,
        "vast.api_key"
            | "kaggle.token"
            | "kaggle.username"
            | "kaggle.key"
            | "mlflow.token"
            | "mlflow.username"
            | "mlflow.password"
    )
}

/// Paths whose runtime type is `Option<f64>` and whose default is also `null`
/// — there is no way to infer "this is a number" from JSON alone, so list
/// them here. Adding a new nullable-numeric field requires editing this list;
/// non-nullable fields and Option fields with concrete defaults are inferred
/// automatically.
const NUMERIC_HINT_PATHS: &[&str] = &["budget.daily_budget_usd", "budget.monthly_budget_usd"];

/// Walk a dotted path on a JSON Value and set the leaf, coercing the input
/// string to the leaf's existing JSON type. `defaults` is consulted when the
/// current value is `null` (typical for unset Option fields). The leaf and
/// every parent must already exist in the schema — unknown keys fail.
fn set_dotted(target: &mut Value, key: &str, raw: &str, defaults: Option<&Value>) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.iter().any(|p| p.is_empty()) {
        bail!("invalid config key: `{key}`");
    }
    let (leaf, parents) = parts
        .split_last()
        .ok_or_else(|| anyhow!("empty config key"))?;

    let mut cur: &mut Value = target;
    for p in parents {
        let map = cur
            .as_object_mut()
            .ok_or_else(|| anyhow!("config path `{key}` traverses a non-object"))?;
        if !map.contains_key(*p) {
            bail!("unknown config key: `{key}`");
        }
        cur = map.get_mut(*p).expect("present");
    }
    let map = cur
        .as_object_mut()
        .ok_or_else(|| anyhow!("config path `{key}` parent is not an object"))?;
    if !map.contains_key(*leaf) {
        bail!("unknown config key: `{key}`");
    }

    let cur_leaf = map.get(*leaf).cloned();
    let hint = match cur_leaf.as_ref() {
        Some(Value::Null) | None => defaults
            .and_then(|d| nav(d, &parts))
            .filter(|v| !v.is_null()),
        Some(other) => Some(other.clone()),
    };

    let coerced =
        coerce_scalar(key, raw, hint.as_ref()).with_context(|| format!("config key `{key}`"))?;
    map.insert(leaf.to_string(), coerced);
    Ok(())
}

/// Set a `vendors.<name>.<field>` entry. Auto-creates the per-vendor entry
/// when missing. `<field>` can be any path inside `VendorDefaults`, including
/// nested keys like `extra.<adapter_specific>` (which lives under a
/// HashMap and is also auto-vivified).
fn cmd_set_vendor(config_dir: &Path, rest: &str, value: &str) -> Result<()> {
    let (name, field) = rest.split_once('.').ok_or_else(|| {
        anyhow!("vendors key must be `vendors.<name>.<field>` (e.g. vendors.vast.default_gpu)")
    })?;
    Vendor::from_str(name).map_err(|e| anyhow!(e))?;

    let cfg = GlobalConfig::load(config_dir)?;
    let mut json = serde_json::to_value(&cfg)?;

    // `vendors` is `skip_serializing_if = HashMap::is_empty`, so the key may
    // be missing from the serialized form on a fresh install.
    let root = json
        .as_object_mut()
        .expect("GlobalConfig serializes to an object");
    let vendors_map = root
        .entry("vendors".to_string())
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .expect("vendors entry is an object");
    let entry = vendors_map
        .entry(name.to_string())
        .or_insert_with(|| serde_json::to_value(VendorDefaults::default()).unwrap());

    // Auto-vivify the `extra.<key>` HashMap entry: same trick as above.
    // `extra` is also `skip_serializing_if`, so the key may be absent.
    if let Some(extra_key) = field.strip_prefix("extra.") {
        let entry_obj = entry
            .as_object_mut()
            .expect("VendorDefaults serializes to an object");
        let extra = entry_obj
            .entry("extra".to_string())
            .or_insert_with(|| Value::Object(Default::default()))
            .as_object_mut()
            .expect("extra entry is an object");
        extra.insert(extra_key.to_string(), Value::String(value.to_string()));
    } else {
        let defaults = serde_json::to_value(VendorDefaults::default())?;
        set_dotted(entry, field, value, Some(&defaults))?;
    }

    let updated: GlobalConfig = serde_json::from_value(json)
        .with_context(|| format!("invalid value for `vendors.{name}.{field}`"))?;
    updated.save(config_dir)?;
    println!("vendors.{name}.{field} = {value}");
    Ok(())
}

fn nav(v: &Value, parts: &[&str]) -> Option<Value> {
    let mut cur = v;
    for p in parts {
        cur = cur.get(*p)?;
    }
    Some(cur.clone())
}

fn coerce_scalar(key: &str, raw: &str, hint: Option<&Value>) -> Result<Value> {
    match hint {
        Some(Value::Bool(_)) => match raw.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(Value::Bool(true)),
            "false" | "0" | "no" | "off" => Ok(Value::Bool(false)),
            _ => bail!("expected boolean (true/false), got: {raw}"),
        },
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => {
            let parsed: i64 = raw.parse().context("expected integer")?;
            Ok(serde_json::json!(parsed))
        }
        Some(Value::Number(_)) => {
            let parsed: f64 = raw.parse().context("expected number")?;
            Ok(serde_json::json!(parsed))
        }
        Some(Value::Array(_)) => {
            // CSV — trimmed, empty entries dropped. Always strings; numeric
            // arrays would round-trip via serde later if any field needed it.
            let items: Vec<String> = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            Ok(serde_json::json!(items))
        }
        Some(Value::Object(_)) => {
            bail!("cannot set nested object at `{key}` via `config set`")
        }
        Some(Value::String(_)) | Some(Value::Null) | None => {
            if NUMERIC_HINT_PATHS.contains(&key) {
                let parsed: f64 = raw.parse().context("expected number")?;
                return Ok(serde_json::json!(parsed));
            }
            Ok(Value::String(raw.to_string()))
        }
    }
}
