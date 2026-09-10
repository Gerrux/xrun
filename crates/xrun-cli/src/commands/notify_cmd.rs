#![deny(unsafe_code)]

//! `xrun notify` — test, send, and inspect push notifications.
//!
//! The poll-daemon sends notifications on its own; this command exists so
//! you can prove the channel works *before* leaving a $2/h instance
//! unattended, and so you can read back what was sent.

use std::path::Path;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use xrun_core::{Credentials, GlobalConfig, Store};
use xrun_notify::{messages, Kind, Notifier, Priority, SendOutcome, Skipped};

#[derive(Args)]
pub struct NotifyArgs {
    #[command(subcommand)]
    pub subcommand: NotifySubcommand,
}

#[derive(Subcommand)]
pub enum NotifySubcommand {
    /// Send a test notification to every configured channel and report
    /// per-channel success. Exit 1 if no channel is configured or any fails.
    Test(TestArgs),
    /// Send an ad-hoc notification (e.g. from a script or an agent).
    Send(SendArgs),
    /// Show the delivery journal (what was sent, where, and whether it worked).
    Log(LogArgs),
    /// List notification kinds accepted by `[notify].events`.
    Kinds(KindsArgs),
}

#[derive(Args)]
pub struct TestArgs {
    /// Only test this channel (must still be listed in `[notify].channels`).
    #[arg(long)]
    pub channel: Option<String>,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SendArgs {
    /// Notification title
    pub title: String,
    /// Body text (optional)
    #[arg(long, short = 'b')]
    pub body: Option<String>,
    /// Attach to a run ID (shows up in `xrun notify log --run`)
    #[arg(long)]
    pub run: Option<String>,
    /// low | default | high | urgent
    #[arg(long, default_value = "default")]
    pub priority: String,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct LogArgs {
    /// Only entries for this run ID
    #[arg(long)]
    pub run: Option<String>,
    /// Max entries (newest first)
    #[arg(long, default_value = "30")]
    pub limit: usize,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct KindsArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Build the notifier the way every caller (daemon, launch, watchdog)
/// should: from `config.toml` + `credentials.toml` on disk. Misconfigured
/// channels are returned as warnings, not errors — a missing Telegram
/// token must not stop a run from launching.
pub fn build_notifier(config_dir: &Path) -> (Notifier, Vec<String>) {
    let global = GlobalConfig::load(config_dir).unwrap_or_default();
    let creds = Credentials::load(config_dir).unwrap_or_default();
    Notifier::from_config(&global.notify, &creds)
}

/// Same, but log the warnings — for the daemon/launch paths where nobody
/// reads the return value.
pub fn build_notifier_logged(config_dir: &Path) -> Notifier {
    let (n, warnings) = build_notifier(config_dir);
    for w in warnings {
        tracing::warn!("notify: {w}");
    }
    n
}

pub fn run(args: &NotifyArgs, db_path: &Path, config_dir: &Path) -> Result<()> {
    match &args.subcommand {
        NotifySubcommand::Test(a) => cmd_test(a, db_path, config_dir),
        NotifySubcommand::Send(a) => cmd_send(a, db_path, config_dir),
        NotifySubcommand::Log(a) => cmd_log(a, db_path),
        NotifySubcommand::Kinds(a) => cmd_kinds(a),
    }
}

fn open_store(db_path: &Path) -> Option<Store> {
    match Store::open(db_path) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!("notify: store unavailable ({e}); skipping journal");
            None
        }
    }
}

fn print_outcome(outcome: &SendOutcome, warnings: &[String], json: bool) -> bool {
    if json {
        let out = serde_json::json!({
            "outcome": outcome,
            "warnings": warnings,
            "ok": outcome.any_ok(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return outcome.any_ok();
    }
    for w in warnings {
        println!("  ! {w}");
    }
    match outcome {
        SendOutcome::Sent(deliveries) => {
            for d in deliveries {
                if d.ok {
                    println!("  ✓ {}", d.channel);
                } else {
                    println!(
                        "  ✗ {}: {}",
                        d.channel,
                        d.error.as_deref().unwrap_or("failed")
                    );
                }
            }
            deliveries.iter().all(|d| d.ok) && !deliveries.is_empty()
        }
        SendOutcome::Skipped(Skipped::NoChannels) => {
            println!(
                "  no channels configured.\n  \
                 xrun config set notify.channels ntfy\n  \
                 xrun config set ntfy.topic <your-topic>"
            );
            false
        }
        SendOutcome::Skipped(reason) => {
            println!("  skipped: {reason:?}");
            false
        }
    }
}

fn cmd_test(args: &TestArgs, db_path: &Path, config_dir: &Path) -> Result<()> {
    let global = GlobalConfig::load(config_dir)?;
    let creds = Credentials::load(config_dir)?;
    let mut cfg = global.notify.clone();
    if let Some(only) = &args.channel {
        if !cfg.channels.iter().any(|c| c == only) {
            bail!(
                "channel `{only}` is not in [notify].channels ({})",
                if cfg.channels.is_empty() {
                    "empty".to_string()
                } else {
                    cfg.channels.join(", ")
                }
            );
        }
        cfg.channels = vec![only.clone()];
    }
    let (notifier, warnings) = Notifier::from_config(&cfg, &creds);
    let names = notifier.channel_names();
    let n = messages::test(&names);
    let mut store = open_store(db_path);
    let outcome = notifier.send_unfiltered(store.as_mut(), &n);
    if !args.json {
        println!("notify test:");
    }
    let ok = print_outcome(&outcome, &warnings, args.json);
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_send(args: &SendArgs, db_path: &Path, config_dir: &Path) -> Result<()> {
    let Some(priority) = Priority::parse(&args.priority) else {
        bail!(
            "invalid --priority `{}` (low|default|high|urgent)",
            args.priority
        );
    };
    let (notifier, warnings) = build_notifier(config_dir);
    let n = messages::manual(
        &args.title,
        args.body.as_deref().unwrap_or(""),
        args.run.as_deref(),
        priority,
    );
    let mut store = open_store(db_path);
    let outcome = notifier.send_unfiltered(store.as_mut(), &n);
    let ok = print_outcome(&outcome, &warnings, args.json);
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_log(args: &LogArgs, db_path: &Path) -> Result<()> {
    let store = Store::open(db_path)?;
    let rows = store.list_notify_log(args.run.as_deref(), args.limit.max(1))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no notifications sent yet");
        return Ok(());
    }
    for r in &rows {
        let mark = if r.ok { "✓" } else { "✗" };
        let run = r
            .run_id
            .as_deref()
            .map(|id| {
                let n = id.len();
                if n > 8 {
                    id[n - 8..].to_string()
                } else {
                    id.to_string()
                }
            })
            .unwrap_or_else(|| "-".into());
        println!(
            "{} {} {:<9} {:<24} {:<8} {}",
            r.ts.format("%m-%d %H:%M"),
            mark,
            r.channel,
            r.kind,
            run,
            r.title
        );
        if let Some(e) = &r.error {
            println!("      {e}");
        }
    }
    Ok(())
}

fn cmd_kinds(args: &KindsArgs) -> Result<()> {
    if args.json {
        let out: Vec<serde_json::Value> = Kind::ALL
            .iter()
            .map(|k| {
                serde_json::json!({
                    "kind": k.as_str(),
                    "priority": k.default_priority().as_str(),
                    "description": k.describe(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("[notify].events accepts \"*\", exact kinds, or prefixes like \"budget.*\":\n");
    for k in Kind::ALL {
        println!(
            "  {:<26} {:<8} {}",
            k.as_str(),
            k.default_priority().as_str(),
            k.describe()
        );
    }
    Ok(())
}
