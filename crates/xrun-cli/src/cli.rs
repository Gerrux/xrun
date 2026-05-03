#![deny(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use crate::commands::{
    config_cmd::ConfigArgs, cp::CpArgs, dataset::DatasetSubcommand, init::InitArgs,
};

#[derive(Parser)]
#[command(name = "xrun", version, about = "ML experiment runner")]
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
    /// Reconcile vendor instances with the local DB and clean up orphans
    Gc(GcArgs),
    /// Open an interactive SSH session on the run's instance
    Shell(ShellArgs),
    /// Re-run a previous run
    Rerun(RerunArgs),
    /// Copy files between instances (or local↔instance) via streaming tar
    #[command(name = "cp")]
    Cp(CpArgs),
    /// Check system health and configuration
    Doctor(DoctorArgs),
    /// Manage xrun configuration
    Config(ConfigArgs),
    /// Show your vast.ai account balance
    Balance(BalanceArgs),
    /// Manage Kaggle datasets (push, status, list)
    Dataset(DatasetArgs),
    /// Reconcile stale `running` runs against the vendor and fix their status
    #[command(name = "fix-status")]
    FixStatus(FixStatusArgs),
    /// Materialise a Cartesian grid of manifests and (optionally) launch them
    Sweep(SweepArgs),
    /// First-run wizard: detect local capabilities, add vendors, choose
    /// logging mode. Spawns the TUI by default; use --non-interactive for
    /// scripted setup or --probe-local for capability detection.
    Init(InitArgs),
    /// Open the interactive TUI (same as running xrun on a TTY with no arguments)
    Tui,
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
    /// Per-instance hard cap (USD). Auto-destroy when accumulated cost exceeds.
    /// Falls back to `[budget].max_cost_per_instance_usd` from config (default $10).
    #[arg(long, value_name = "USD")]
    pub max_cost: Option<f64>,
    /// Per-instance hard cap (hours). Auto-destroy after this lifetime.
    /// Falls back to `[budget].max_lifetime_hours` from config (default 8).
    #[arg(long, value_name = "HOURS")]
    pub max_hours: Option<f64>,
    /// Idle timeout (minutes). 0 disables. Falls back to `[budget].idle_timeout_min` (default 30).
    #[arg(long, value_name = "MIN")]
    pub idle_timeout: Option<f64>,
    /// Skip the billable-action confirm prompt (required when stdin is not a TTY).
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Reuse an existing live vast instance instead of provisioning a new one.
    /// Skips offer search + create_instance. Pass either a vast instance ID
    /// (numeric) or an xrun run ID (ULID) — the latter resolves to its
    /// instance and inherits the SSH handle.
    #[arg(long, value_name = "ID")]
    pub reuse_instance: Option<String>,
    /// Provision + upload, then stop without executing the run.cmd. Useful for
    /// staging data on a long-lived instance that you'll resume later.
    #[arg(long)]
    pub upload_only: bool,
    /// Override a manifest run-arg without editing YAML, e.g.
    /// `--override run.args.--lr=5e-4`. Repeatable. Same syntax as
    /// `xrun rerun --patch`.
    #[arg(long = "override", value_name = "PATH=VALUE")]
    pub overrides: Vec<String>,
    /// Print every external command we shell out to (vastai, ssh, tar, ...)
    /// before running it. Use when something fails opaquely and you need to
    /// see the exact invocation.
    #[arg(long)]
    pub trace: bool,
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
    /// Stream remote stdout live via SSH (tail -F)
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
    /// Follow events (not yet implemented — use `xrun logs -f` for live stdout)
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
    /// Render metrics as a PNG chart and save to this path
    #[arg(long, value_name = "PATH")]
    pub png: Option<PathBuf>,
    /// Print the MLflow run URL and exit (requires mlflow.url in config)
    #[arg(long)]
    pub mlflow_url: bool,
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
    /// Stop all active runs (and destroy their instances)
    #[arg(long)]
    pub all: bool,
    /// Destroy instance immediately without graceful stop
    #[arg(long)]
    pub force: bool,
    /// Keep the vendor instance alive (for debugging)
    #[arg(long)]
    pub keep_instance: bool,
}

#[derive(Args)]
pub struct ShellArgs {
    /// Run ID (ULID) or vast instance ID. Defaults to the single active run.
    pub id: Option<String>,
    /// Run a single command and exit, instead of an interactive shell.
    #[arg(long, short = 'c')]
    pub cmd: Option<String>,
}

#[derive(Args)]
pub struct GcArgs {
    /// Show what would be destroyed without acting
    #[arg(long)]
    pub dry_run: bool,
    /// Also destroy instances that exist on the vendor but are not in our DB
    #[arg(long)]
    pub include_unknown: bool,
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
pub struct BalanceArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct DoctorArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
    /// Validate one or more manifest files (parse + schema check + Kaggle resource checks).
    /// Failures here are fatal exit 1 even with --json.
    #[arg(long = "manifest", value_name = "PATH")]
    pub manifests: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// xrun dataset subcommand
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct DatasetPushArgs {
    /// Local directory to upload (must contain the dataset files)
    pub local_dir: PathBuf,
    /// Kaggle dataset slug in owner/name format (e.g. kartaviychert/my-dataset)
    #[arg(long)]
    pub slug: String,
    /// Version message for subsequent pushes
    #[arg(long, short = 'm')]
    pub message: Option<String>,
    /// Wait for the dataset to become ready before exiting (default: true)
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    pub wait: bool,
}

#[derive(Args)]
pub struct DatasetStatusArgs {
    /// Kaggle dataset slug (owner/name)
    pub slug: String,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct DatasetListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct DatasetArgs {
    #[command(subcommand)]
    pub subcommand: DatasetSubcommand,
}

#[derive(Args)]
pub struct FixStatusArgs {
    /// Run ID (ULID) to check. Omit to reconcile all runs in `running` status.
    pub id: Option<String>,
    /// Show what would change without writing to the DB
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct SweepArgs {
    /// Path to the base manifest YAML file
    pub manifest: PathBuf,
    /// Grid axis: PATH=v1,v2,...  Repeatable; multiplies the search space.
    /// Example: --grid run.args.--lr=1e-3,5e-4,1e-4
    #[arg(long, value_name = "PATH=V1,V2,...")]
    pub grid: Vec<String>,
    /// Output directory for materialised manifests.
    /// Defaults to exp/sweep_<stem>_<timestamp>/.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Launch each materialised manifest after writing.
    #[arg(long)]
    pub launch: bool,
    /// Detach each launched run (only valid with --launch).
    #[arg(long)]
    pub detach: bool,
    /// Skip the billable-action confirm (only valid with --launch).
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Print the plan without writing files or launching.
    #[arg(long)]
    pub dry_run: bool,
    /// Output as JSON (machine-readable plan).
    #[arg(long)]
    pub json: bool,
}
