#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::backend::Backend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xrun_core::config::Credentials;
use xrun_core::vendor::VendorAdapter;
use xrun_core::{GlobalConfig, Store};

use crate::event::DataUpdate;
use crate::services::live::LiveService;
use crate::services::vendor_probe::{ProbeRequest, VendorProbeService};
use crate::state::{AppState, Modal, Screen};
use crate::theme::Theme;
use crate::view;

mod actions;
mod keys;
mod loaders;
mod vendor;

pub struct App {
    pub(super) store: Store,
    pub(super) config: GlobalConfig,
    pub(super) state: AppState,
    pub(super) data_rx: mpsc::Receiver<DataUpdate>,
    pub(super) data_tx: mpsc::Sender<DataUpdate>,
    pub(super) db_path: Option<PathBuf>,
    pub(super) config_dir: Option<PathBuf>,
    pub(super) live_shutdown: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) probe_tx: Option<std::sync::mpsc::Sender<ProbeRequest>>,
    pub(super) probe_shutdown: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl App {
    pub fn new(store: Store, config: GlobalConfig) -> Self {
        let theme = Theme::from_name(&config.tui.theme);
        let mut state = AppState::new(theme);
        state.default_vendor_name = config.defaults.vendor.as_ref().map(vendor_name);
        let (data_tx, data_rx) = mpsc::channel(256);
        Self {
            store,
            config,
            state,
            data_rx,
            data_tx,
            db_path: None,
            config_dir: None,
            live_shutdown: None,
            probe_tx: None,
            probe_shutdown: None,
        }
    }

    pub fn with_db_path(mut self, db_path: PathBuf) -> Self {
        self.db_path = Some(db_path);
        self
    }

    pub fn with_config_dir(mut self, config_dir: PathBuf) -> Self {
        self.config_dir = Some(config_dir);
        self
    }

    pub(super) fn save_config(&self) {
        if let Some(dir) = &self.config_dir {
            if let Err(e) = self.config.save(dir) {
                tracing::warn!("failed to save config: {}", e);
            }
        }
    }

    pub fn data_sender(&self) -> mpsc::Sender<DataUpdate> {
        self.data_tx.clone()
    }

    pub async fn run(mut self, cancel: CancellationToken) -> Result<()> {
        let db_path_for_live = self.db_path.clone();
        if let Some(db_path) = db_path_for_live {
            let live = LiveService::new(db_path, self.data_tx.clone());
            self.live_shutdown = Some(live.shutdown_flag());
            live.start();
        }

        self.reload_credentials();
        self.start_vendor_probe();

        // Always show splash; first-run gets 1500ms, returning users 600ms.
        let splash_ms: u64 = if self.state.credentials.is_empty() {
            1500
        } else {
            600
        };
        let now = Instant::now();
        self.state.modal = Some(Modal::Splash {
            started_at: now,
            deadline: now + Duration::from_millis(splash_ms),
        });
        self.state.dirty = true;

        let mut terminal = ratatui::init();
        let result = self.event_loop(&mut terminal, cancel).await;

        if let Some(shutdown) = &self.live_shutdown {
            shutdown.store(true, Ordering::Relaxed);
        }
        if let Some(shutdown) = &self.probe_shutdown {
            shutdown.store(true, Ordering::Relaxed);
        }
        ratatui::restore();
        result
    }

    fn reload_credentials(&mut self) {
        if let Some(dir) = &self.config_dir {
            match Credentials::load(dir) {
                Ok(creds) => self.state.credentials = creds,
                Err(e) => tracing::warn!("failed to load credentials: {}", e),
            }
        }
    }

    pub(super) fn start_vendor_probe(&mut self) {
        let adapters = self.build_adapters();
        if adapters.is_empty() {
            return;
        }
        let svc = VendorProbeService::new(adapters, self.data_tx.clone());
        self.probe_shutdown = Some(svc.shutdown_flag());
        self.probe_tx = Some(svc.command_sender());
        svc.start();
    }

    fn build_adapters(&self) -> Vec<(String, Box<dyn VendorAdapter + Send>)> {
        let mut adapters: Vec<(String, Box<dyn VendorAdapter + Send>)> = Vec::new();
        if self.state.credentials.vast.api_key.is_some() {
            if let Some(db_path) = self.db_path_clone() {
                if let Ok(store) = Store::open(&db_path) {
                    let adapter =
                        xrun_vast::VastAdapter::new(self.state.credentials.vast.clone(), store);
                    adapters.push(("vast".to_string(), Box::new(adapter)));
                }
            }
        }
        adapters
    }

    pub(super) fn db_path_clone(&self) -> Option<PathBuf> {
        self.db_path
            .clone()
            .or_else(|| xrun_core::paths::data_dir().ok().map(|d| d.join("xrun.db")))
    }

    pub(super) fn trigger_probe(&self, vendor: Option<&str>) {
        if let Some(tx) = &self.probe_tx {
            let req = match vendor {
                Some(v) => ProbeRequest::Vendor(v.to_string()),
                None => ProbeRequest::All,
            };
            let _ = tx.send(req);
        }
    }

    pub(crate) async fn event_loop<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.reload_runs()?;

        let mut event_stream = EventStream::new();
        let render_interval = Duration::from_millis(100);
        let mut last_render = Instant::now();

        loop {
            self.maybe_dismiss_splash();
            if self.state.dirty || last_render.elapsed() >= render_interval {
                self.state.runs.throbber_frame = self.state.runs.throbber_frame.wrapping_add(1);
                self.state.anim_frame = self.state.anim_frame.wrapping_add(1);
                terminal.draw(|f| view::render(f, &self.state))?;
                self.state.dirty = false;
                last_render = Instant::now();
            }

            let timeout = render_interval.saturating_sub(last_render.elapsed());

            tokio::select! {
                biased;
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(CrosstermEvent::Key(key))) => {
                            // On Windows crossterm fires events for both Press and Release;
                            // ignore everything except Press so each keystroke acts once.
                            if key.kind != KeyEventKind::Press {
                                continue;
                            }
                            if self.handle_key(key)? {
                                return Ok(());
                            }
                            if let Some(path) = self.state.editor_path.take() {
                                self.open_editor(terminal, &path)?;
                            }
                        }
                        Some(Err(e)) => return Err(e.into()),
                        None => return Ok(()),
                        _ => {}
                    }
                }
                maybe_update = self.data_rx.recv() => {
                    if let Some(update) = maybe_update {
                        self.on_data_update(update);
                    }
                }
                () = tokio::time::sleep(timeout) => {}
                () = cancel.cancelled() => {
                    return Ok(());
                }
            }
        }
    }

    fn on_data_update(&mut self, update: DataUpdate) {
        match update {
            DataUpdate::RunCreated(_) | DataUpdate::RunStatusChanged(_, _) => {
                if let Err(e) = self.reload_runs() {
                    tracing::error!("failed to reload runs: {}", e);
                }
            }
            DataUpdate::EventsAppended(run_id, _) | DataUpdate::LogsAppended(run_id, _) => {
                if let Screen::RunDetail(current_id, _) = &self.state.screen {
                    if *current_id == run_id {
                        let id = run_id.clone();
                        if let Err(e) = self.load_run_detail(&id) {
                            tracing::error!("failed to reload run detail: {}", e);
                        }
                    }
                }
            }
            DataUpdate::VendorStatusUpdated(name) => {
                if let Some(s) = crate::services::vendor_probe::latest::read(&name) {
                    self.state.vendor_statuses.insert(name, s);
                }
            }
            DataUpdate::VendorInstancesUpdated(name) => {
                if let Some(insts) = crate::services::vendor_probe::latest::read_instances(&name) {
                    if name == "vast" {
                        self.state.instances.remote = insts;
                    }
                }
            }
            _ => {}
        }
        self.state.dirty = true;
    }

    fn maybe_dismiss_splash(&mut self) {
        let expired = matches!(
            &self.state.modal,
            Some(Modal::Splash { deadline, .. }) if Instant::now() >= *deadline
        );
        if expired {
            self.state.modal = None;
            self.state.dirty = true;
        }
    }
}

pub(super) fn vendor_name(v: &xrun_core::manifest::types::Vendor) -> String {
    match v {
        xrun_core::manifest::types::Vendor::Vast => "vast".to_string(),
        xrun_core::manifest::types::Vendor::Kaggle => "kaggle".to_string(),
        xrun_core::manifest::types::Vendor::Local => "local".to_string(),
        xrun_core::manifest::types::Vendor::Ssh => "ssh".to_string(),
    }
}
