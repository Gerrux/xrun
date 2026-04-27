#![deny(unsafe_code)]

mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::config_cmd::ConfigArgs;

#[derive(Parser)]
#[command(name = "xrun", version = "0.1.0", about = "ML experiment runner")]
struct Cli {
    /// Override config directory (for testing; also read from XRUN_CONFIG_DIR)
    #[arg(long, hide = true, env = "XRUN_CONFIG_DIR", global = true)]
    config_dir: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage xrun configuration
    Config(ConfigArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_dir = match cli.config_dir {
        Some(d) => d,
        None => xrun_core::paths::config_dir()?,
    };

    match cli.command {
        Some(Commands::Config(args)) => commands::config_cmd::run(&args, &config_dir)?,
        None => {}
    }
    Ok(())
}
