use anyhow::Result;
use ratatui::backend::Backend;
use ratatui::Terminal;
use xrun_core::{ListFilter, RunId, RunStatus};

use crate::state::{
    LaunchManifest, LogPaneState, RunDetailState, RunSection, Screen, SettingsState,
};

use super::{vendor_name, App};

impl App {
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

    pub(super) fn handle_rerun(&mut self, run_id: RunId) -> Result<()> {
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

    pub(super) fn open_editor<B: Backend>(
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
}
