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

    // Skip stderr tracing when launching the TUI: the alternate screen would
    // be corrupted by log lines, and any tracing event during the TUI session
    // (e.g. a config save warning) would render on top of the UI.
    let tui_mode = is_tui_invocation(&cli.command);
    if !tui_mode {
        init_tracing(cli.verbose, cli.quiet);
    }

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
            let config_dir = get_config()?;
            if args.dry_run {
                xrun_cli::commands::launch::run(
                    &args,
                    &PathBuf::new(),
                    &PathBuf::new(),
                    &config_dir,
                )?;
            } else {
                let ctx = get_data_ctx()?;
                xrun_cli::commands::launch::run(&args, &ctx.db_path, &ctx.runs_dir, &config_dir)?;
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
            xrun_cli::commands::logs::run(&args, &ctx.db_path, &ctx.runs_dir)?;
        }
        Some(Commands::Events(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::events_cmd::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Metrics(args)) => {
            let ctx = get_data_ctx()?;
            let config_dir = get_config()?;
            xrun_cli::commands::metrics_cmd::run(&args, &ctx.db_path, &config_dir)?;
        }
        Some(Commands::Pull(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::pull::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Stop(args)) => {
            let ctx = get_data_ctx()?;
            let config_dir = get_config()?;
            xrun_cli::commands::stop::run(&args, &ctx.db_path, &ctx.runs_dir, &config_dir)?;
        }
        Some(Commands::Gc(args)) => {
            let ctx = get_data_ctx()?;
            let config_dir = get_config()?;
            xrun_cli::commands::gc::run(&args, &ctx.db_path, &config_dir)?;
        }
        Some(Commands::Shell(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::shell::run(&args, &ctx.db_path)?;
        }
        Some(Commands::Rerun(args)) => {
            let ctx = get_data_ctx()?;
            let config_dir = get_config()?;
            xrun_cli::commands::rerun::run(&args, &ctx.db_path, &ctx.runs_dir, &config_dir)?;
        }
        Some(Commands::Cp(args)) => {
            let config_dir = get_config()?;
            xrun_cli::commands::cp::run(&args, &config_dir)?;
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
            let config_dir = get_config()?;
            let runs_dir = args.runs_dir.clone().unwrap_or(ctx.runs_dir);
            xrun_cli::commands::poll_daemon::run(&args, &ctx.db_path, &runs_dir, &config_dir)?;
        }
        Some(Commands::Balance(args)) => {
            let config_dir = get_config()?;
            xrun_cli::commands::balance::run(&args, &config_dir)?;
        }
        Some(Commands::Dataset(args)) => {
            let config_dir = get_config()?;
            xrun_cli::commands::dataset::run(&args.subcommand, &config_dir)?;
        }
        Some(Commands::FixStatus(args)) => {
            let ctx = get_data_ctx()?;
            let config_dir = get_config()?;
            xrun_cli::commands::fix_status::run(&args, &ctx.db_path, &ctx.runs_dir, &config_dir)?;
        }
        Some(Commands::Resume(args)) => {
            let ctx = get_data_ctx()?;
            let config_dir = get_config()?;
            xrun_cli::commands::resume::run(&args, &ctx.db_path, &ctx.runs_dir, &config_dir)?;
        }
        Some(Commands::Sweep(args)) => {
            let ctx = get_data_ctx()?;
            let config_dir = get_config()?;
            xrun_cli::commands::sweep::run(&args, &ctx.db_path, &ctx.runs_dir, &config_dir)?;
        }
        Some(Commands::Diff(args)) => {
            let ctx = get_data_ctx()?;
            xrun_cli::commands::diff::run(&args, &ctx.db_path, &ctx.runs_dir)?;
        }
        Some(Commands::Init(args)) => {
            let config_dir = get_config()?;
            xrun_cli::commands::init::run(&args, &config_dir)?;
        }
        Some(Commands::Tui) => {
            #[cfg(feature = "tui")]
            {
                let ctx = get_data_ctx()?;
                let config_dir = get_config()?;
                let config = xrun_core::GlobalConfig::load(&config_dir).unwrap_or_default();
                xrun_tui::launch(ctx.db_path, config, config_dir)?;
            }
            #[cfg(not(feature = "tui"))]
            {
                eprintln!("error: TUI support not compiled in (build with --features tui)");
                std::process::exit(1);
            }
        }
        None => {
            use std::io::IsTerminal;
            if std::io::stdout().is_terminal() {
                let status = std::process::Command::new("xrun-tui")
                    .status()
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "failed to launch xrun-tui: {e}\n\
                             Install with: pip install -e python/xrun_tui"
                        )
                    })?;
                std::process::exit(status.code().unwrap_or(1));
            }
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

fn is_tui_invocation(command: &Option<Commands>) -> bool {
    use std::io::IsTerminal;
    match command {
        None => std::io::stdout().is_terminal(),
        #[cfg(feature = "tui")]
        Some(Commands::Tui) => true,
        _ => false,
    }
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
