#![allow(dead_code)]

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::backend::Backend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xrun_core::{GlobalConfig, ListFilter, RunId, RunStatus, Store};

use crate::event::DataUpdate;
use crate::screens::instances::{self as instances_screen, InstancesAction};
use crate::screens::launch::{self as launch_screen, LaunchAction};
use crate::screens::run_detail::{self as run_detail_screen, RunDetailAction};
use crate::screens::runs::{self as runs_screen, RunsAction};
use crate::screens::settings::{self as settings_screen, SettingsAction};
use crate::state::{
    AppState, ConfirmAction, LaunchManifest, LogPaneState, Modal, RunDetailState, Screen,
    SettingsState, Tab,
};
use crate::theme::Theme;
use crate::view;

pub struct App {
    store: Store,
    config: GlobalConfig,
    state: AppState,
    data_rx: mpsc::Receiver<DataUpdate>,
    data_tx: mpsc::Sender<DataUpdate>,
}

impl App {
    pub fn new(store: Store, config: GlobalConfig) -> Self {
        let theme = Theme::from_name(&config.tui.theme);
        let state = AppState::new(theme);
        let (data_tx, data_rx) = mpsc::channel(256);
        Self {
            store,
            config,
            state,
            data_rx,
            data_tx,
        }
    }

    pub fn data_sender(&self) -> mpsc::Sender<DataUpdate> {
        self.data_tx.clone()
    }

    pub async fn run(mut self, cancel: CancellationToken) -> Result<()> {
        let mut terminal = ratatui::init();
        let result = self.event_loop(&mut terminal, cancel).await;
        ratatui::restore();
        result
    }

    pub(crate) async fn event_loop<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        cancel: CancellationToken,
    ) -> Result<()> {
        use std::time::{Duration, Instant};

        self.reload_runs()?;

        let mut event_stream = EventStream::new();
        let render_interval = Duration::from_millis(100);
        let mut last_render = Instant::now();

        loop {
            if self.state.dirty || last_render.elapsed() >= render_interval {
                self.state.runs.throbber_frame = self.state.runs.throbber_frame.wrapping_add(1);
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
            self.state.modal = None;
            self.state.dirty = true;
            return Ok(false);
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
        }
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
                self.state.dirty = true;
            }
            SettingsAction::SavePollIntervalActive(v) => {
                self.config.poller.interval_active_secs = v;
                self.state.settings.poll_interval_active = v;
                self.state.dirty = true;
            }
            SettingsAction::SavePollIntervalIdle(v) => {
                self.config.poller.interval_idle_secs = v;
                self.state.settings.poll_interval_idle = v;
                self.state.dirty = true;
            }
            SettingsAction::SaveDefaultVendor(vendor) => {
                self.config.defaults.vendor = vendor.as_deref().and_then(|s| {
                    let quoted = format!("\"{}\"", s);
                    serde_json::from_str(&quoted).ok()
                });
                self.state.settings.default_vendor = vendor.unwrap_or_default();
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
        }
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
                .map(|v| format!("{:?}", v).to_lowercase())
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
                let dst = exp_dir.join(format!("{}-rerun-{}.yaml", run.name, ts));
                std::fs::copy(src, &dst)?;
                tracing::info!("copied manifest to {}", dst.display());
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

        self.state.dirty = true;
        Ok(())
    }

    fn on_data_update(&mut self, update: DataUpdate) {
        match update {
            DataUpdate::RunCreated(_) | DataUpdate::RunStatusChanged(_) => {
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
            _ => {}
        }
        self.state.dirty = true;
    }
}
