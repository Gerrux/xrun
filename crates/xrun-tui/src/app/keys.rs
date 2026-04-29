use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::screens::instances::{self as instances_screen};
use crate::screens::launch::{self as launch_screen};
use crate::screens::palette::{self as palette_screen, PaletteAction};
use crate::screens::run_detail::{self as run_detail_screen};
use crate::screens::runs::{self as runs_screen};
use crate::screens::settings::{self as settings_screen, SETTINGS_ROW_COUNT};
use crate::screens::vendors::{self as vendors_screen};
use crate::state::{ConfirmAction, InstancesSection, Modal, RunSection, Screen, Tab};
use crate::theme::Theme;

use super::App;

impl App {
    pub(super) fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Result<bool> {
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
                self.state.theme = Theme::from_name(&name);
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
                    InstancesSection::Local => self.state.instances.instances.len(),
                    InstancesSection::Remote => self.state.instances.remote.len(),
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
}
