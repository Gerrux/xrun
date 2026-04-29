#![deny(unsafe_code)]

pub mod app;
pub mod event;
pub mod screens;
pub mod services;
pub mod state;
pub mod theme;
pub mod view;

use std::path::PathBuf;

use anyhow::Result;
use xrun_core::GlobalConfig;

/// Synchronous entry-point: creates a tokio runtime and runs the TUI.
/// Called from xrun-cli when no subcommand is given on a TTY, or via `xrun tui`.
pub fn launch(db_path: PathBuf, config: GlobalConfig, config_dir: PathBuf) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let store = xrun_core::Store::open(&db_path)?;
        let cancel = tokio_util::sync::CancellationToken::new();
        app::App::new(store, config)
            .with_db_path(db_path)
            .with_config_dir(config_dir)
            .run(cancel)
            .await
    })
}
