#![deny(unsafe_code)]

use std::path::PathBuf;

use anyhow::Result;
use xrun_cli::cli::{Cli, Commands};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    use clap::Parser;
    let cli = Cli::parse();

    init_tracing(cli.verbose, cli.quiet);

    let config_dir_override = cli.config_dir.clone();
    let data_dir_override = cli.data_dir.clone();
    let db_override = cli.db.clone();

    let get_config = move || -> Result<PathBuf> {
        match config_dir_override {
            Some(d) => Ok(d),
            None => Ok(xrun_core::paths::config_dir()?),
        }
    };

    let get_data_ctx = move || -> Result<DataCtx> {
        let data_dir = match data_dir_override.clone() {
            Some(d) => d,
            None => xrun_core::paths::data_dir()?,
        };
        let db_path = match db_override.clone() {
            Some(p) => p,
            None => data_dir.join("runs.db"),
        };
        let runs_dir = data_dir.join("runs");
        Ok(DataCtx { db_path, runs_dir })
    };

    match cli.command {
        Some(Commands::Launch(args)) => {
            if args.dry_run {
                xrun_cli::commands::launch::run(&args, &PathBuf::new(), &PathBuf::new())?;
            } else {
                let ctx = get_data_ctx()?;
                xrun_cli::commands::launch::run(&args, &ctx.db_path, &ctx.runs_dir)?;
            }
        }
        Some(Commands::Ls(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::ls::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Show(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::show::run(&args, &ctx.db_path, &ctx.runs_dir)?;
        }
        Some(Commands::Logs(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::logs::run(&args, &ctx.runs_dir)?;
        }
        Some(Commands::Events(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::events_cmd::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Metrics(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::metrics_cmd::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Pull(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::pull::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Stop(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::stop::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Rerun(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::rerun::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Doctor(args)) => {
            let config_dir = get_config()?;
            let db_path_opt = get_data_ctx().ok().map(|c| c.db_path);
            xrun_cli::commands::doctor::run(&args, &config_dir, db_path_opt.as_deref())?;
        }
        Some(Commands::Config(args)) => {
            let config_dir = get_config()?;
            xrun_cli::commands::config_cmd::run(&args, &config_dir)?;
        }
        Some(Commands::PollDaemon(args)) => {
            let ctx = get_data_ctx()?;
            let runs_dir = args.runs_dir.clone().unwrap_or(ctx.runs_dir);
            xrun_cli::commands::poll_daemon::run(&args, &ctx.db_path, &runs_dir)?;
        }
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
        }
    }

    Ok(())
}

struct DataCtx {
    db_path: PathBuf,
    runs_dir: PathBuf,
}

fn init_tracing(verbose: bool, quiet: bool) {
    let level = if verbose {
        "debug"
    } else if quiet {
        "warn"
    } else {
        "info"
    };
    let filter = tracing_subscriber::EnvFilter::try_new(level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
