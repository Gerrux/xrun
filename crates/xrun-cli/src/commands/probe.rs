//! `xrun config probe` — verify connectivity to a vendor / sink with the
//! credentials passed in *for this invocation only* (no disk writes).
//!
//! Designed for the first-run wizard: lets the TUI confirm pasted keys
//! actually authenticate before persisting them to `credentials.toml`.
//!
//! Credentials come from environment variables so they don't appear in
//! `ps` output:
//!
//! | vendor  | env vars                                                                  |
//! |---------|---------------------------------------------------------------------------|
//! | vast    | `XRUN_PROBE_VAST_KEY`                                                     |
//! | kaggle  | `XRUN_PROBE_KAGGLE_USERNAME` + `XRUN_PROBE_KAGGLE_KEY` *or* `..._TOKEN`   |
//! | mlflow  | `XRUN_PROBE_MLFLOW_TOKEN` *or* `..._USERNAME` + `..._PASSWORD`            |
//! | ssh     | (none — uses `--ssh-host` / `--ssh-user` / `--ssh-port` / `--ssh-key`)    |
//! | local   | (none)                                                                    |
//!
//! Output is always one JSON object on stdout:
//!
//! ```json
//! { "vendor": "vast", "ok": true, "detail": "...", "elapsed_ms": 234 }
//! ```
//!
//! Exit code is 0 even on probe failure — `ok: false` is the signal. This
//! keeps the wizard in control of how to react.

#![deny(unsafe_code)]

use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Args;
use serde_json::json;

use xrun_mlflow::{Auth, MlflowClient};
use xrun_ssh::cmd::SshConn;
use xrun_ssh::ssh::ssh_exec;

#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// Vendor / sink to probe: vast, kaggle, mlflow, ssh, local.
    #[arg(long)]
    pub vendor: String,

    /// MLflow tracking URL (required when --vendor mlflow).
    #[arg(long)]
    pub mlflow_url: Option<String>,

    /// SSH host (required when --vendor ssh).
    #[arg(long)]
    pub ssh_host: Option<String>,

    /// SSH user (required when --vendor ssh).
    #[arg(long)]
    pub ssh_user: Option<String>,

    /// SSH port (default 22).
    #[arg(long)]
    pub ssh_port: Option<u16>,

    /// SSH private key path (optional; tilde-expanded).
    #[arg(long)]
    pub ssh_key: Option<String>,
}

pub fn run(args: &ProbeArgs) -> Result<()> {
    let started = Instant::now();
    let (ok, detail) = match args.vendor.as_str() {
        "vast" => probe_vast(),
        "kaggle" => probe_kaggle(),
        "mlflow" => probe_mlflow(args.mlflow_url.as_deref()),
        "ssh" => probe_ssh(args),
        "local" => probe_local(),
        other => (false, format!("unknown vendor: {other}")),
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    println!(
        "{}",
        json!({
            "vendor": args.vendor,
            "ok": ok,
            "detail": detail,
            "elapsed_ms": elapsed_ms,
        })
    );
    Ok(())
}

// ── vast ────────────────────────────────────────────────────────────────────

fn probe_vast() -> (bool, String) {
    let Some(key) = env_nonempty("XRUN_PROBE_VAST_KEY") else {
        return (false, "missing XRUN_PROBE_VAST_KEY".into());
    };
    let rt = match build_rt() {
        Ok(rt) => rt,
        Err(e) => return (false, e),
    };
    match rt.block_on(xrun_vast::rest::show_user(&key)) {
        Ok(info) => match info.effective_balance() {
            Some(b) => (true, format!("authenticated; balance ${b:.2}")),
            None => (true, "authenticated".into()),
        },
        Err(e) => (false, format!("{e}")),
    }
}

// ── kaggle ──────────────────────────────────────────────────────────────────

fn probe_kaggle() -> (bool, String) {
    let token = env_nonempty("XRUN_PROBE_KAGGLE_TOKEN");
    let username = env_nonempty("XRUN_PROBE_KAGGLE_USERNAME");
    let key = env_nonempty("XRUN_PROBE_KAGGLE_KEY");

    if token.is_some() && (username.is_some() || key.is_some()) {
        return (
            false,
            "set either KAGGLE_TOKEN or KAGGLE_USERNAME+KAGGLE_KEY, not both".into(),
        );
    }
    if username.is_some() ^ key.is_some() {
        return (
            false,
            "KAGGLE_USERNAME and KAGGLE_KEY must be set together".into(),
        );
    }
    if token.is_none() && username.is_none() {
        return (false, "no Kaggle credentials in env".into());
    }

    // JWT can't be validated against the public REST endpoint reliably — it
    // is only consumed by the kernel runtime. Report honestly and let the
    // wizard surface that as a warn.
    if token.is_some() {
        return (
            true,
            "JWT token staged — actual validation happens at first launch".into(),
        );
    }

    let rt = match build_rt() {
        Ok(rt) => rt,
        Err(e) => return (false, e),
    };
    let username = username.unwrap();
    let key = key.unwrap();
    rt.block_on(async {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
        {
            Ok(c) => c,
            Err(e) => return (false, format!("http client: {e}")),
        };
        // `/api/v1/competitions/list?page=1` is the cheapest authenticated
        // endpoint that returns 401/403 on bad creds.
        let resp = client
            .get("https://www.kaggle.com/api/v1/competitions/list?page=1")
            .basic_auth(&username, Some(&key))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => (true, format!("authenticated as {username}")),
            Ok(r)
                if r.status() == reqwest::StatusCode::UNAUTHORIZED
                    || r.status() == reqwest::StatusCode::FORBIDDEN =>
            {
                (
                    false,
                    format!(
                        "rejected by Kaggle ({}: bad username or key)",
                        r.status().as_u16()
                    ),
                )
            }
            Ok(r) => (false, format!("HTTP {}", r.status().as_u16())),
            Err(e) => (false, format!("network: {e}")),
        }
    })
}

// ── mlflow ──────────────────────────────────────────────────────────────────

fn probe_mlflow(url: Option<&str>) -> (bool, String) {
    let Some(url) = url.filter(|s| !s.is_empty()) else {
        return (false, "missing --mlflow-url".into());
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return (false, "URL must start with http:// or https://".into());
    }
    let token = env_nonempty("XRUN_PROBE_MLFLOW_TOKEN");
    let username = env_nonempty("XRUN_PROBE_MLFLOW_USERNAME");
    let password = env_nonempty("XRUN_PROBE_MLFLOW_PASSWORD");

    if token.is_some() && (username.is_some() || password.is_some()) {
        return (
            false,
            "set either MLFLOW_TOKEN or MLFLOW_USERNAME+MLFLOW_PASSWORD, not both".into(),
        );
    }
    if username.is_some() ^ password.is_some() {
        return (
            false,
            "MLFLOW_USERNAME and MLFLOW_PASSWORD must be set together".into(),
        );
    }

    let auth = match (token, username, password) {
        (Some(t), _, _) => Some(Auth::Bearer(t)),
        (None, Some(u), Some(p)) => Some(Auth::Basic {
            username: u,
            password: p,
        }),
        _ => None,
    };
    let rt = match build_rt() {
        Ok(rt) => rt,
        Err(e) => return (false, e),
    };
    rt.block_on(async {
        let client = MlflowClient::new(url, auth);
        // Hits `experiments/get-by-name?experiment_name=__xrun_probe__`. A
        // valid server returns 404 (experiment doesn't exist) without erroring;
        // the MlflowClient maps that to NotFound, which we treat as success.
        match client.get_or_create_experiment("__xrun_probe__").await {
            Ok(_id) => (true, format!("connected to {url}")),
            Err(xrun_mlflow::MlflowError::NotFound(_)) => (true, format!("connected to {url}")),
            Err(e) => (false, format!("{e}")),
        }
    })
}

// ── ssh ─────────────────────────────────────────────────────────────────────

fn probe_ssh(args: &ProbeArgs) -> (bool, String) {
    let host = match args.ssh_host.as_deref().filter(|s| !s.is_empty()) {
        Some(h) => h.to_string(),
        None => return (false, "missing --ssh-host".into()),
    };
    let user = match args.ssh_user.as_deref().filter(|s| !s.is_empty()) {
        Some(u) => u.to_string(),
        None => return (false, "missing --ssh-user".into()),
    };
    let port = args.ssh_port.unwrap_or(22);
    let key = args
        .ssh_key
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(expand_tilde)
        .map(std::path::PathBuf::from);

    let conn = SshConn {
        alias: "probe".into(),
        host,
        user,
        port,
        key,
    };
    match ssh_exec(&conn, "true") {
        Ok(_) => (true, format!("connected to {}", conn.target())),
        Err(e) => {
            let msg = e.to_string();
            // Strip multiline ssh stderr to one line for the JSON detail.
            let one_line: String = msg.lines().take(3).collect::<Vec<_>>().join(" / ");
            (false, one_line)
        }
    }
}

fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    p.to_string()
}

fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

// ── local ───────────────────────────────────────────────────────────────────

fn probe_local() -> (bool, String) {
    let gpus = xrun_local::process::probe_gpu_summary();
    if gpus.is_empty() {
        (true, format!("CPU only on {}", std::env::consts::OS))
    } else {
        (true, format!("{} GPU(s): {}", gpus.len(), gpus.join(", ")))
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn build_rt() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("async runtime: {e}"))
}
