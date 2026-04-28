#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::backend::Backend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xrun_core::config::Credentials;
use xrun_core::vendor::VendorAdapter;
use xrun_core::{GlobalConfig, ListFilter, RunId, RunStatus, Store};

use crate::event::DataUpdate;
use crate::screens::instances::{self as instances_screen, InstancesAction};
use crate::screens::launch::{self as launch_screen, LaunchAction};
use crate::screens::palette::{self as palette_screen, PaletteAction};
use crate::screens::run_detail::{self as run_detail_screen, RunDetailAction};
use crate::screens::runs::{self as runs_screen, RunsAction};
use crate::screens::settings::{self as settings_screen, SettingsAction, SETTINGS_ROW_COUNT};
use crate::screens::vendors::{self as vendors_screen, VendorsAction};
use crate::services::live::LiveService;
use crate::services::vendor_probe::{ProbeRequest, VendorProbeService};
use crate::state::{
    AppState, ConfirmAction, EditField, LaunchManifest, LogPaneState, Modal, RunDetailState,
    RunSection, Screen, SettingsState, Tab,
};
use crate::theme::Theme;
use crate::view;

pub struct App {
    store: Store,
    config: GlobalConfig,
    state: AppState,
    data_rx: mpsc::Receiver<DataUpdate>,
    data_tx: mpsc::Sender<DataUpdate>,
    db_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    live_shutdown: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    probe_tx: Option<std::sync::mpsc::Sender<ProbeRequest>>,
    probe_shutdown: Option<Arc<std::sync::atomic::AtomicBool>>,
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

    fn save_config(&self) {
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

    fn start_vendor_probe(&mut self) {
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

    fn db_path_clone(&self) -> Option<PathBuf> {
        self.db_path
            .clone()
            .or_else(|| xrun_core::paths::data_dir().ok().map(|d| d.join("xrun.db")))
    }

    fn trigger_probe(&self, vendor: Option<&str>) {
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

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Result<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            ratatui::restore();
            std::process::exit(130);
        }

        if matches!(&self.state.modal, Some(Modal::Confirm { .. })) {
            self.state.g_pressed = false;
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.state.modal = None;
                    self.state.dirty = true;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(Modal::Confirm { action, .. }) = self.state.modal.take() {
                        self.execute_confirm_action(action)?;
                    }
                }
                _ => {}
            }
            return Ok(false);
        }

        if matches!(&self.state.modal, Some(Modal::Help)) {
            self.state.g_pressed = false;
            self.state.modal = None;
            self.state.dirty = true;
            return Ok(false);
        }

        if matches!(&self.state.modal, Some(Modal::Splash { .. })) {
            self.state.g_pressed = false;
            self.state.modal = None;
            self.state.dirty = true;
            // First-run shortcut: any keypress on splash drops the user into
            // Vendors so they can configure credentials.
            if self.state.credentials.is_empty() {
                self.state.push_screen(Screen::Vendors);
            }
            return Ok(false);
        }

        if matches!(&self.state.modal, Some(Modal::CommandPalette { .. })) {
            return self.handle_palette_key(key);
        }

        if matches!(&self.state.modal, Some(Modal::VendorEdit { .. })) {
            return self.handle_vendor_edit_key(key);
        }

        // Global bindings: ? and :
        match key.code {
            KeyCode::Char('?') => {
                self.state.g_pressed = false;
                self.state.modal = Some(Modal::Help);
                self.state.dirty = true;
                return Ok(false);
            }
            KeyCode::Char(':') => {
                self.state.g_pressed = false;
                let completions = palette_screen::compute_completions("");
                self.state.modal = Some(Modal::CommandPalette {
                    input: String::new(),
                    completions,
                    selected_completion: 0,
                });
                self.state.dirty = true;
                return Ok(false);
            }
            _ => {}
        }

        // g g / G navigation
        match key.code {
            KeyCode::Char('G') => {
                self.state.g_pressed = false;
                self.navigate_bottom();
                return Ok(false);
            }
            KeyCode::Char('g') => {
                if self.state.g_pressed {
                    self.state.g_pressed = false;
                    self.navigate_top();
                } else {
                    self.state.g_pressed = true;
                }
                return Ok(false);
            }
            _ => {
                self.state.g_pressed = false;
            }
        }

        let screen = self.state.screen.clone();
        match screen {
            Screen::Runs => {
                let action = runs_screen::handle_key(&mut self.state, key);
                self.handle_runs_action(action)
            }
            Screen::RunDetail(_, _) => {
                let action = run_detail_screen::handle_key(&mut self.state, key);
                self.handle_run_detail_action(action)
            }
            Screen::Launch => {
                let action = launch_screen::handle_key(&mut self.state, key);
                self.handle_launch_action(action)
            }
            Screen::Instances => {
                let action = instances_screen::handle_key(&mut self.state, key);
                self.handle_instances_action(action)
            }
            Screen::Settings => {
                let action = settings_screen::handle_key(&mut self.state, key);
                self.handle_settings_action(action)
            }
            Screen::Vendors => {
                let action = vendors_screen::handle_key(&mut self.state, key);
                self.handle_vendors_action(action)
            }
        }
    }

    fn handle_palette_key(&mut self, key: crossterm::event::KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.state.modal = None;
                self.state.dirty = true;
            }
            KeyCode::Enter => {
                let cmd = if let Some(Modal::CommandPalette { input, .. }) = self.state.modal.take()
                {
                    input
                } else {
                    String::new()
                };
                return self.execute_palette_command(&cmd);
            }
            KeyCode::Tab => {
                if let Some(Modal::CommandPalette {
                    ref completions,
                    ref mut selected_completion,
                    ref mut input,
                }) = self.state.modal
                {
                    if !completions.is_empty() {
                        let next = (*selected_completion + 1) % completions.len();
                        let new_input = completions[next].clone();
                        *selected_completion = next;
                        *input = new_input;
                    }
                }
                self.state.dirty = true;
            }
            KeyCode::Backspace => {
                if let Some(Modal::CommandPalette {
                    ref mut input,
                    ref mut completions,
                    ref mut selected_completion,
                }) = self.state.modal
                {
                    input.pop();
                    let new_comps = palette_screen::compute_completions(input.as_str());
                    *completions = new_comps;
                    *selected_completion = 0;
                }
                self.state.dirty = true;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(Modal::CommandPalette {
                    ref mut input,
                    ref mut completions,
                    ref mut selected_completion,
                }) = self.state.modal
                {
                    input.push(c);
                    let new_comps = palette_screen::compute_completions(input.as_str());
                    *completions = new_comps;
                    *selected_completion = 0;
                }
                self.state.dirty = true;
            }
            _ => {}
        }
        Ok(false)
    }

    fn execute_palette_command(&mut self, cmd: &str) -> Result<bool> {
        let action = palette_screen::parse_command(cmd, &self.state);
        match action {
            PaletteAction::Quit => return Ok(true),
            PaletteAction::GotoScreen(screen) => match screen {
                Screen::Runs => {
                    self.state.screen_stack.clear();
                    self.state.screen = Screen::Runs;
                    self.state.dirty = true;
                }
                Screen::Instances => {
                    self.load_instances()?;
                    self.state.push_screen(Screen::Instances);
                }
                Screen::Settings => {
                    self.load_settings();
                    self.state.push_screen(Screen::Settings);
                }
                Screen::Vendors => {
                    self.state.push_screen(Screen::Vendors);
                    self.trigger_probe(None);
                }
                other => {
                    self.state.push_screen(other);
                }
            },
            PaletteAction::ShowLaunchConfirm(path) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Launch manifest '{}'?", path),
                    action: ConfirmAction::LaunchRun(path),
                });
                self.state.dirty = true;
            }
            PaletteAction::ShowStopConfirm(run_id, id_str) => {
                let name = run_id
                    .as_ref()
                    .and_then(|id| {
                        self.state
                            .runs
                            .active_runs
                            .iter()
                            .find(|r| r.id == *id)
                            .map(|r| r.name.clone())
                    })
                    .unwrap_or_else(|| id_str.clone());
                if let Some(id) = run_id {
                    self.state.modal = Some(Modal::Confirm {
                        message: format!("Stop run '{}'?", name),
                        action: ConfirmAction::StopRun(id),
                    });
                    self.state.dirty = true;
                }
            }
            PaletteAction::ShowPullConfirm(run_id, id_str) => {
                let name = run_id
                    .as_ref()
                    .and_then(|id| {
                        self.state
                            .runs
                            .recent_runs
                            .iter()
                            .find(|r| r.id == *id)
                            .map(|r| r.name.clone())
                    })
                    .unwrap_or_else(|| id_str.clone());
                if let Some(id) = run_id {
                    self.state.modal = Some(Modal::Confirm {
                        message: format!("Pull best checkpoint for '{}'?", name),
                        action: ConfirmAction::PullRun(id),
                    });
                    self.state.dirty = true;
                }
            }
            PaletteAction::OpenRunDetail(id_str) => {
                let run_id = self
                    .state
                    .runs
                    .active_runs
                    .iter()
                    .chain(self.state.runs.recent_runs.iter())
                    .find(|r| r.id.to_string() == id_str || r.name == id_str)
                    .map(|r| r.id.clone());
                if let Some(id) = run_id {
                    self.load_run_detail(&id)?;
                    self.state.push_screen(Screen::RunDetail(id, Tab::Stages));
                }
            }
            PaletteAction::Rerun(run_id, _) => {
                if let Some(id) = run_id {
                    self.handle_rerun(id)?;
                }
            }
            PaletteAction::ApplyTheme(name) => {
                self.config.tui.theme = name.clone();
                self.state.theme = crate::theme::Theme::from_name(&name);
                self.state.settings.theme = name;
                self.save_config();
                self.state.dirty = true;
            }
            PaletteAction::Nothing => {}
        }
        Ok(false)
    }

    fn navigate_top(&mut self) {
        match &self.state.screen {
            Screen::Runs => {
                self.state.runs.selected = 0;
            }
            Screen::Launch => {
                self.state.launch.selected = 0;
            }
            Screen::Instances => {
                self.state.instances.selected = 0;
            }
            Screen::Settings => {
                self.state.settings.selected_row = 0;
            }
            Screen::Vendors => {
                self.state.vendors.selected = 0;
            }
            Screen::RunDetail(_, _) => {}
        }
        self.state.dirty = true;
    }

    fn navigate_bottom(&mut self) {
        match self.state.screen.clone() {
            Screen::Runs => {
                let len = match self.state.runs.section {
                    RunSection::Active => self.state.runs.active_runs.len(),
                    RunSection::Recent => self.state.runs.recent_runs.len(),
                };
                if len > 0 {
                    self.state.runs.selected = len - 1;
                }
            }
            Screen::Launch => {
                let len = self.state.launch.manifests.len();
                if len > 0 {
                    self.state.launch.selected = len - 1;
                }
            }
            Screen::Instances => {
                let len = match self.state.instances.section {
                    crate::state::InstancesSection::Local => self.state.instances.instances.len(),
                    crate::state::InstancesSection::Remote => self.state.instances.remote.len(),
                };
                if len > 0 {
                    self.state.instances.selected = len - 1;
                }
            }
            Screen::Settings => {
                self.state.settings.selected_row = SETTINGS_ROW_COUNT - 1;
            }
            Screen::Vendors => {
                let len = self.state.vendors.vendors.len();
                if len > 0 {
                    self.state.vendors.selected = len - 1;
                }
            }
            Screen::RunDetail(_, _) => {}
        }
        self.state.dirty = true;
    }

    fn handle_runs_action(&mut self, action: RunsAction) -> Result<bool> {
        match action {
            RunsAction::OpenRunDetail(id) => {
                self.load_run_detail(&id)?;
                self.state.push_screen(Screen::RunDetail(id, Tab::Stages));
            }
            RunsAction::OpenLaunch => {
                self.load_launch_manifests()?;
                self.state.push_screen(Screen::Launch);
            }
            RunsAction::OpenInstances => {
                self.load_instances()?;
                self.state.push_screen(Screen::Instances);
            }
            RunsAction::OpenSettings => {
                self.load_settings();
                self.state.push_screen(Screen::Settings);
            }
            RunsAction::OpenVendors => {
                self.state.push_screen(Screen::Vendors);
                self.trigger_probe(None);
            }
            RunsAction::ShowStopConfirm(id, name) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Stop run '{}'?", name),
                    action: ConfirmAction::StopRun(id),
                });
                self.state.dirty = true;
            }
            RunsAction::ShowPullConfirm(id, name) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Pull best checkpoint for '{}'?", name),
                    action: ConfirmAction::PullRun(id),
                });
                self.state.dirty = true;
            }
            RunsAction::Rerun(id) => {
                self.handle_rerun(id)?;
            }
            RunsAction::Quit => return Ok(true),
            RunsAction::Nothing => {}
        }
        Ok(false)
    }

    fn handle_run_detail_action(&mut self, action: RunDetailAction) -> Result<bool> {
        match action {
            RunDetailAction::Back => {
                self.state.pop_screen();
            }
            RunDetailAction::SwitchTab(tab) => {
                if let Screen::RunDetail(id, _) = &self.state.screen {
                    let id = id.clone();
                    self.state.screen = Screen::RunDetail(id, tab);
                    self.state.dirty = true;
                }
            }
            RunDetailAction::OpenEditor(path) => {
                self.state.editor_path = Some(path);
            }
            RunDetailAction::ToggleAutoscroll => {
                self.state.run_detail.log.autoscroll = !self.state.run_detail.log.autoscroll;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollUp => {
                self.state.run_detail.log.scroll =
                    self.state.run_detail.log.scroll.saturating_sub(1);
                self.state.run_detail.log.autoscroll = false;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollDown => {
                self.state.run_detail.log.scroll =
                    self.state.run_detail.log.scroll.saturating_add(1);
                self.state.run_detail.log.autoscroll = false;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollTop => {
                self.state.run_detail.log.scroll = 0;
                self.state.run_detail.log.autoscroll = false;
                self.state.dirty = true;
            }
            RunDetailAction::ScrollBottom => {
                self.state.run_detail.log.scroll = usize::MAX;
                self.state.run_detail.log.autoscroll = true;
                self.state.dirty = true;
            }
            RunDetailAction::Nothing => {}
        }
        Ok(false)
    }

    fn handle_launch_action(&mut self, action: LaunchAction) -> Result<bool> {
        match action {
            LaunchAction::Confirm(path) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Launch manifest '{}'?", path),
                    action: ConfirmAction::LaunchRun(path),
                });
                self.state.dirty = true;
            }
            LaunchAction::Back => {
                self.state.pop_screen();
            }
            LaunchAction::Nothing => {}
        }
        Ok(false)
    }

    fn handle_instances_action(&mut self, action: InstancesAction) -> Result<bool> {
        match action {
            InstancesAction::ShowDestroyConfirm(id) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Destroy orphan instance '{}'?", id),
                    action: ConfirmAction::DestroyInstance(id),
                });
                self.state.dirty = true;
            }
            InstancesAction::Back => {
                self.state.pop_screen();
            }
            InstancesAction::Nothing => {}
        }
        Ok(false)
    }

    fn handle_settings_action(&mut self, action: SettingsAction) -> Result<bool> {
        match action {
            SettingsAction::SaveTheme(name) => {
                self.config.tui.theme = name.clone();
                self.state.theme = Theme::from_name(&name);
                self.state.settings.theme = name;
                self.save_config();
                self.state.dirty = true;
            }
            SettingsAction::SavePollIntervalActive(v) => {
                self.config.poller.interval_active_secs = v;
                self.state.settings.poll_interval_active = v;
                self.save_config();
                self.state.dirty = true;
            }
            SettingsAction::SavePollIntervalIdle(v) => {
                self.config.poller.interval_idle_secs = v;
                self.state.settings.poll_interval_idle = v;
                self.save_config();
                self.state.dirty = true;
            }
            SettingsAction::SaveDefaultVendor(vendor) => {
                let trimmed = vendor.as_deref().map(str::trim).unwrap_or("");
                let parsed = match trimmed.to_ascii_lowercase().as_str() {
                    "" => Some(None),
                    "vast" => Some(Some(xrun_core::manifest::types::Vendor::Vast)),
                    "kaggle" => Some(Some(xrun_core::manifest::types::Vendor::Kaggle)),
                    _ => None,
                };
                if let Some(v) = parsed {
                    self.config.defaults.vendor = v;
                    self.state.settings.default_vendor = trimmed.to_ascii_lowercase();
                    self.save_config();
                } else {
                    tracing::warn!("ignoring unknown vendor '{}'", trimmed);
                }
                self.state.dirty = true;
            }
            SettingsAction::Back => {
                self.state.pop_screen();
            }
            SettingsAction::Nothing => {}
        }
        Ok(false)
    }

    fn execute_confirm_action(&mut self, action: ConfirmAction) -> Result<()> {
        match action {
            ConfirmAction::StopRun(id) => {
                self.store.update_run_status(&id, RunStatus::Cancelled)?;
                self.reload_runs()?;
            }
            ConfirmAction::PullRun(id) => {
                tracing::info!("pull requested for run {}", id);
            }
            ConfirmAction::DestroyInstance(instance_id) => {
                self.store
                    .update_instance_destroyed(&instance_id, chrono::Utc::now())?;
                self.load_instances()?;
            }
            ConfirmAction::LaunchRun(path) => {
                tracing::info!("launch requested for manifest {}", path);
                self.state.pop_screen();
                self.reload_runs()?;
            }
            ConfirmAction::RevokeVendor(name) => {
                self.revoke_vendor(&name)?;
            }
        }
        Ok(())
    }

    fn handle_vendors_action(&mut self, action: VendorsAction) -> Result<bool> {
        match action {
            VendorsAction::Back => {
                self.state.pop_screen();
            }
            VendorsAction::OpenEdit(vendor) => {
                self.open_vendor_edit(&vendor);
            }
            VendorsAction::ImportNative(vendor) => {
                self.import_native_vendor(&vendor);
            }
            VendorsAction::TestConnection(vendor) => {
                self.trigger_probe(Some(&vendor));
                self.state.vendors.flash = Some(format!("probing {}...", vendor));
                self.state.dirty = true;
            }
            VendorsAction::ShowRevokeConfirm(vendor) => {
                self.state.modal = Some(Modal::Confirm {
                    message: format!("Revoke credentials for '{}'?", vendor),
                    action: ConfirmAction::RevokeVendor(vendor),
                });
                self.state.dirty = true;
            }
            VendorsAction::Nothing => {}
        }
        Ok(false)
    }

    fn open_vendor_edit(&mut self, vendor: &str) {
        let fields = match vendor {
            "vast" => vec![EditField {
                label: "api_key".to_string(),
                value: self
                    .state
                    .credentials
                    .vast
                    .api_key
                    .clone()
                    .unwrap_or_default(),
                secret: true,
            }],
            "kaggle" => vec![
                EditField {
                    label: "username".to_string(),
                    value: self
                        .state
                        .credentials
                        .kaggle
                        .username
                        .clone()
                        .unwrap_or_default(),
                    secret: false,
                },
                EditField {
                    label: "key".to_string(),
                    value: self
                        .state
                        .credentials
                        .kaggle
                        .key
                        .clone()
                        .unwrap_or_default(),
                    secret: true,
                },
            ],
            "mlflow" => vec![
                EditField {
                    label: "url".to_string(),
                    value: self.config.mlflow.url.clone().unwrap_or_default(),
                    secret: false,
                },
                EditField {
                    label: "token".to_string(),
                    value: self
                        .state
                        .credentials
                        .mlflow
                        .token
                        .clone()
                        .unwrap_or_default(),
                    secret: true,
                },
            ],
            _ => return,
        };
        self.state.modal = Some(Modal::VendorEdit {
            vendor: vendor.to_string(),
            fields,
            focus: 0,
            flash: None,
        });
        self.state.dirty = true;
    }

    fn import_native_vendor(&mut self, vendor: &str) {
        let result: Result<String, String> = match vendor {
            "vast" => match Credentials::import_vast_native() {
                Ok(Some(token)) => {
                    self.state.credentials.vast.api_key = Some(token);
                    Ok("imported vast api_key from ~/.config/vastai/vast_api_key".to_string())
                }
                Ok(None) => Err("native vast key file not found or empty".to_string()),
                Err(e) => Err(format!("read failed: {}", e)),
            },
            "kaggle" => match Credentials::import_kaggle_native() {
                Ok(Some((u, k))) => {
                    self.state.credentials.kaggle.username = Some(u);
                    self.state.credentials.kaggle.key = Some(k);
                    Ok("imported kaggle.json".to_string())
                }
                Ok(None) => Err("kaggle.json not found or missing fields".to_string()),
                Err(e) => Err(format!("read failed: {}", e)),
            },
            other => Err(format!("no native import for vendor '{}'", other)),
        };

        match result {
            Ok(msg) => {
                if let Err(e) = self.persist_credentials() {
                    self.state.vendors.flash = Some(format!("save failed: {}", e));
                } else {
                    self.state.vendors.flash = Some(msg);
                    self.refresh_vendor_probe();
                    self.trigger_probe(Some(vendor));
                }
            }
            Err(e) => {
                self.state.vendors.flash = Some(e);
            }
        }
        self.state.dirty = true;
    }

    fn revoke_vendor(&mut self, vendor: &str) -> Result<()> {
        match vendor {
            "vast" => self.state.credentials.vast.api_key = None,
            "kaggle" => {
                self.state.credentials.kaggle.username = None;
                self.state.credentials.kaggle.key = None;
            }
            "mlflow" => self.state.credentials.mlflow.token = None,
            _ => {}
        }
        self.persist_credentials().map_err(anyhow::Error::from)?;
        self.state.vendor_statuses.remove(vendor);
        self.state.vendors.flash = Some(format!("revoked {}", vendor));
        self.refresh_vendor_probe();
        self.state.dirty = true;
        Ok(())
    }

    fn persist_credentials(&self) -> std::io::Result<()> {
        let Some(dir) = &self.config_dir else {
            return Ok(());
        };
        self.state
            .credentials
            .save(dir)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    fn refresh_vendor_probe(&mut self) {
        if let Some(flag) = &self.probe_shutdown {
            flag.store(true, Ordering::Relaxed);
        }
        self.probe_shutdown = None;
        self.probe_tx = None;
        self.start_vendor_probe();
    }

    fn handle_vendor_edit_key(&mut self, key: crossterm::event::KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                if let Some(Modal::VendorEdit { fields, .. }) = self.state.modal.as_mut() {
                    // wipe secrets from in-memory modal state on close
                    for f in fields.iter_mut() {
                        if f.secret {
                            f.value.clear();
                        }
                    }
                }
                self.state.modal = None;
                self.state.dirty = true;
            }
            KeyCode::Tab | KeyCode::Down => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if !fields.is_empty() {
                        *focus = (*focus + 1) % fields.len();
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::BackTab | KeyCode::Up => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if !fields.is_empty() {
                        *focus = if *focus == 0 {
                            fields.len() - 1
                        } else {
                            *focus - 1
                        };
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.value.pop();
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(Modal::VendorEdit { fields, focus, .. }) = self.state.modal.as_mut() {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.value.push(c);
                        self.state.dirty = true;
                    }
                }
            }
            KeyCode::Enter => {
                self.commit_vendor_edit()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn commit_vendor_edit(&mut self) -> Result<()> {
        let Some(Modal::VendorEdit { vendor, fields, .. }) = self.state.modal.take() else {
            return Ok(());
        };
        match vendor.as_str() {
            "vast" => {
                let key = fields
                    .iter()
                    .find(|f| f.label == "api_key")
                    .map(|f| f.value.clone());
                self.state.credentials.vast.api_key = key.filter(|s| !s.is_empty());
            }
            "kaggle" => {
                let user = fields
                    .iter()
                    .find(|f| f.label == "username")
                    .map(|f| f.value.clone());
                let key = fields
                    .iter()
                    .find(|f| f.label == "key")
                    .map(|f| f.value.clone());
                self.state.credentials.kaggle.username = user.filter(|s| !s.is_empty());
                self.state.credentials.kaggle.key = key.filter(|s| !s.is_empty());
            }
            "mlflow" => {
                let url = fields
                    .iter()
                    .find(|f| f.label == "url")
                    .map(|f| f.value.clone());
                let token = fields
                    .iter()
                    .find(|f| f.label == "token")
                    .map(|f| f.value.clone());
                self.config.mlflow.url = url.filter(|s| !s.is_empty());
                self.state.credentials.mlflow.token = token.filter(|s| !s.is_empty());
                self.save_config();
            }
            _ => {}
        }
        if let Err(e) = self.persist_credentials() {
            self.state.vendors.flash = Some(format!("save failed: {}", e));
        } else {
            self.state.vendors.flash = Some(format!("saved {} credentials", vendor));
            self.refresh_vendor_probe();
            self.trigger_probe(Some(&vendor));
        }
        self.state.dirty = true;
        Ok(())
    }

    pub(crate) fn load_launch_manifests(&mut self) -> Result<()> {
        let all_runs = self.store.list_runs(&ListFilter::default())?;
        let run_paths: std::collections::HashSet<String> =
            all_runs.iter().map(|r| r.manifest_path.clone()).collect();

        let exp_dir = std::env::current_dir()
            .unwrap_or_default()
            .join(self.config.defaults.exp_dir.as_deref().unwrap_or("exp"));

        let mut manifests = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&exp_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("yaml")
                    || path.extension().and_then(|e| e.to_str()) == Some("yml")
                {
                    let name = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let content = std::fs::read_to_string(&path).unwrap_or_default();
                    let path_str = path.to_string_lossy().to_string();
                    let previously_run = run_paths.contains(&path_str);
                    manifests.push(LaunchManifest {
                        path,
                        name,
                        content,
                        previously_run,
                    });
                }
            }
        }
        manifests.sort_by(|a, b| a.name.cmp(&b.name));

        self.state.launch.manifests = manifests;
        self.state.launch.selected = 0;
        self.state.dirty = true;
        Ok(())
    }

    pub(crate) fn load_instances(&mut self) -> Result<()> {
        self.state.instances.instances = self.store.list_instances()?;
        self.state.instances.selected = 0;
        // Pull cached vendor instances (filled by probe service) so the Remote
        // tab is non-empty on first render after a successful probe.
        if let Some(insts) = crate::services::vendor_probe::latest::read_instances("vast") {
            self.state.instances.remote = insts;
        }
        // Trigger an immediate refresh so user sees fresh data on Tab.
        self.trigger_probe(Some("vast"));
        self.state.dirty = true;
        Ok(())
    }

    pub(crate) fn load_settings(&mut self) {
        self.state.settings = SettingsState {
            selected_row: 0,
            editing: false,
            edit_input: String::new(),
            theme: self.config.tui.theme.clone(),
            poll_interval_active: self.config.poller.interval_active_secs,
            poll_interval_idle: self.config.poller.interval_idle_secs,
            default_vendor: self
                .config
                .defaults
                .vendor
                .as_ref()
                .map(vendor_name)
                .unwrap_or_default(),
        };
        self.state.dirty = true;
    }

    fn handle_rerun(&mut self, run_id: xrun_core::RunId) -> Result<()> {
        let run = self
            .state
            .runs
            .active_runs
            .iter()
            .chain(self.state.runs.recent_runs.iter())
            .find(|r| r.id == run_id)
            .cloned();

        if let Some(run) = run {
            let src = std::path::Path::new(&run.manifest_path);
            if src.exists() {
                let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
                let exp_dir = std::env::current_dir()?.join("exp");
                std::fs::create_dir_all(&exp_dir)?;
                let safe_name: String = run
                    .name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let safe_name = if safe_name.is_empty() || safe_name.starts_with('.') {
                    format!("run_{}", safe_name.trim_start_matches('.'))
                } else {
                    safe_name
                };
                let dst = exp_dir.join(format!("{}-rerun-{}.yaml", safe_name, ts));
                std::fs::copy(src, &dst)?;
                tracing::info!("copied manifest to {}", dst.display());
            } else {
                tracing::warn!(
                    "rerun: manifest '{}' not found, opening launch picker without copy",
                    run.manifest_path
                );
            }
            self.state.push_screen(Screen::Launch);
        }
        Ok(())
    }

    pub(crate) fn load_run_detail(&mut self, run_id: &RunId) -> Result<()> {
        let run = self.store.get_run(run_id)?;
        let events = self.store.list_events(run_id)?;

        let log_lines = xrun_core::paths::runs_dir()
            .ok()
            .map(|d| d.join(run_id.to_string()).join("stdout.log"))
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .map(|s| s.lines().map(|l| l.to_string()).collect::<Vec<_>>())
            .unwrap_or_default();

        let manifest_text = run
            .as_ref()
            .filter(|r| !r.manifest_path.is_empty())
            .and_then(|r| std::fs::read_to_string(&r.manifest_path).ok())
            .unwrap_or_default();

        self.state.run_detail = RunDetailState {
            run,
            events,
            log: LogPaneState {
                lines: log_lines,
                scroll: usize::MAX,
                autoscroll: true,
                search: None,
            },
            manifest_text,
        };

        self.state.dirty = true;
        Ok(())
    }

    fn open_editor<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        path: &std::path::Path,
    ) -> Result<()> {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

        let _ = std::process::Command::new(&editor).arg(path).status();

        let _ = crossterm::terminal::enable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen);
        terminal.clear()?;

        if let Screen::RunDetail(run_id, _) = &self.state.screen {
            let run_id = run_id.clone();
            let _ = self.load_run_detail(&run_id);
        }
        self.state.dirty = true;
        Ok(())
    }

    pub(crate) fn reload_runs(&mut self) -> Result<()> {
        let all = self.store.list_runs(&ListFilter::default())?;

        self.state.runs.active_runs = all
            .iter()
            .filter(|r| {
                matches!(
                    r.status,
                    RunStatus::Provisioning | RunStatus::Uploading | RunStatus::Running
                )
            })
            .cloned()
            .collect();

        self.state.runs.recent_runs = all
            .iter()
            .filter(|r| {
                matches!(
                    r.status,
                    RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled
                )
            })
            .take(10)
            .cloned()
            .collect();

        let current_len = match self.state.runs.section {
            RunSection::Active => self.state.runs.active_runs.len(),
            RunSection::Recent => self.state.runs.recent_runs.len(),
        };
        self.state.runs.selected = if current_len == 0 {
            0
        } else {
            self.state.runs.selected.min(current_len - 1)
        };

        self.state.dirty = true;
        Ok(())
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
        let expired = matches!(&self.state.modal, Some(Modal::Splash { deadline, .. }) if Instant::now() >= *deadline);
        if expired {
            self.state.modal = None;
            self.state.dirty = true;
        }
    }
}

fn vendor_name(v: &xrun_core::manifest::types::Vendor) -> String {
    match v {
        xrun_core::manifest::types::Vendor::Vast => "vast".to_string(),
        xrun_core::manifest::types::Vendor::Kaggle => "kaggle".to_string(),
    }
}
