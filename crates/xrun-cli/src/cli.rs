#![deny(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use crate::commands::config_cmd::ConfigArgs;

#[derive(Parser)]
#[command(name = "xrun", version = "0.1.0", about = "ML experiment runner")]
pub struct Cli {
    /// Enable debug logging
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,
    /// Suppress all output except errors
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,
    /// Override SQLite database path
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,
    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,
    /// Override config directory
    #[arg(long, hide = true, env = "XRUN_CONFIG_DIR", global = true)]
    pub config_dir: Option<PathBuf>,
    /// Override data directory (used for runs/ and default DB location)
    #[arg(long, hide = true, env = "XRUN_DATA_DIR", global = true)]
    pub data_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Parse, validate, and launch a manifest on a vendor
    Launch(LaunchArgs),
    /// List runs
    Ls(LsArgs),
    /// Show details of a run
    Show(ShowArgs),
    /// Show stdout log of a run
    Logs(LogsArgs),
    /// Show stage events for a run
    Events(EventsArgs),
    /// Show metrics for a run
    Metrics(MetricsArgs),
    /// Pull artifacts from a run to local disk
    Pull(PullArgs),
    /// Stop a running run
    Stop(StopArgs),
    /// Re-run a previous run
    Rerun(RerunArgs),
    /// Check system health and configuration
    Doctor(DoctorArgs),
    /// Manage xrun configuration
    Config(ConfigArgs),
    /// Internal: run the poller in daemon mode for a detached run (hidden)
    #[command(name = "__poll-daemon", hide = true)]
    PollDaemon(PollDaemonArgs),
}

#[derive(Args)]
pub struct LaunchArgs {
    /// Path to the manifest YAML file
    pub manifest: PathBuf,
    /// Print the execution plan without launching anything
    #[arg(long)]
    pub dry_run: bool,
    /// Allow launching even if a run with the same hash already exists
    #[arg(long)]
    pub allow_duplicate: bool,
    /// Override the run name (does not affect the manifest hash)
    #[arg(long)]
    pub name: Option<String>,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
    /// Detach after launch: spawn a background poller daemon and exit immediately
    #[arg(long)]
    pub detach: bool,
}

#[derive(Args)]
pub struct PollDaemonArgs {
    /// Run ID to poll
    pub run_id: String,
    /// Runs directory (passed by the launcher when spawning the daemon)
    #[arg(long, hide = true)]
    pub runs_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct LsArgs {
    /// Show all runs instead of just active + last 10 completed
    #[arg(long)]
    pub all: bool,
    /// Filter by vendor (vast, kaggle)
    #[arg(long)]
    pub vendor: Option<String>,
    /// Filter by status (provisioning, uploading, running, done, failed, cancelled)
    #[arg(long)]
    pub status: Option<String>,
    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,
    /// Show exp/ manifests not yet launched (not implemented in v0.1)
    #[arg(long)]
    pub manifests: bool,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ShowArgs {
    /// Run ID (ULID)
    pub id: String,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct LogsArgs {
    /// Run ID (ULID)
    pub id: String,
    /// Filter lines matching this pattern
    #[arg(long)]
    pub grep: Option<String>,
    /// Follow log output (not supported in v0.1)
    #[arg(long, short = 'f')]
    pub follow: bool,
}

#[derive(Args)]
pub struct EventsArgs {
    /// Run ID (ULID)
    pub id: String,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
    /// Follow events (not supported in v0.1)
    #[arg(long, short = 'f')]
    pub follow: bool,
}

#[derive(Args)]
pub struct MetricsArgs {
    /// Run ID (ULID)
    pub id: String,
    /// Comma-separated metric keys to show
    #[arg(long)]
    pub key: Option<String>,
    /// Print ASCII chart (not implemented in v0.1)
    #[arg(long)]
    pub ascii: bool,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct PullArgs {
    /// Run ID (ULID)
    pub id: Option<String>,
    /// Checkpoint selection: latest, best, all, or glob
    #[arg(long, default_value = "latest")]
    pub ckpt: String,
    /// Pull all artifact patterns from manifest
    #[arg(long)]
    pub artifacts: bool,
    /// Local directory to download into
    #[arg(long)]
    pub into: Option<PathBuf>,
}

#[derive(Args)]
pub struct StopArgs {
    /// Run ID (ULID)
    pub id: Option<String>,
    /// Destroy instance immediately without graceful stop
    #[arg(long)]
    pub force: bool,
    /// Keep the vendor instance alive (for debugging)
    #[arg(long)]
    pub keep_instance: bool,
}

#[derive(Args)]
pub struct RerunArgs {
    /// Run ID (ULID) to repeat
    pub id: String,
    /// Patch a run parameter (jq-style path, e.g. run.args.--lr=5e-4)
    #[arg(long)]
    pub patch: Vec<String>,
}

#[derive(Args)]
pub struct DoctorArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}
