use std::path::PathBuf;

use xrun_core::{Run, RunId, StoredEvent};

use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub enum Tab {
    Stages,
    Logs,
    Manifest,
}

#[derive(Debug, Clone)]
pub struct LogPaneState {
    pub lines: Vec<String>,
    pub scroll: usize,
    pub autoscroll: bool,
    pub search: Option<String>,
}

impl Default for LogPaneState {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            scroll: 0,
            autoscroll: true,
            search: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunDetailState {
    pub run: Option<Run>,
    pub events: Vec<StoredEvent>,
    pub log: LogPaneState,
    pub manifest_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Runs,
    RunDetail(RunId, Tab),
    Launch,
    Instances,
    Settings,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    StopRun(RunId),
    PullRun(RunId),
    DestroyInstance(String),
}

#[derive(Debug, Clone)]
pub enum Modal {
    Confirm {
        message: String,
        action: ConfirmAction,
    },
    FilterInput {
        input: String,
    },
    Help,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum RunSection {
    #[default]
    Active,
    Recent,
}

#[derive(Debug, Clone, Default)]
pub struct RunsState {
    pub active_runs: Vec<Run>,
    pub recent_runs: Vec<Run>,
    pub section: RunSection,
    pub selected: usize,
    pub filter: Option<String>,
    pub throbber_frame: u8,
}

pub struct AppState {
    pub screen: Screen,
    pub screen_stack: Vec<Screen>,
    pub theme: Theme,
    pub modal: Option<Modal>,
    pub dirty: bool,
    pub runs: RunsState,
    pub run_detail: RunDetailState,
    pub editor_path: Option<PathBuf>,
}

impl AppState {
    pub fn new(theme: Theme) -> Self {
        Self {
            screen: Screen::Runs,
            screen_stack: Vec::new(),
            theme,
            modal: None,
            dirty: true,
            runs: RunsState::default(),
            run_detail: RunDetailState::default(),
            editor_path: None,
        }
    }

    pub fn push_screen(&mut self, screen: Screen) {
        let current = std::mem::replace(&mut self.screen, screen);
        self.screen_stack.push(current);
        self.dirty = true;
    }

    pub fn pop_screen(&mut self) {
        if let Some(prev) = self.screen_stack.pop() {
            self.screen = prev;
            self.dirty = true;
        }
    }
}
