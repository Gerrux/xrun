use xrun_core::RunId;

use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub enum Tab {
    Stages,
    Logs,
    Manifest,
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
    Help,
}

pub struct AppState {
    pub screen: Screen,
    pub screen_stack: Vec<Screen>,
    pub theme: Theme,
    pub modal: Option<Modal>,
    pub dirty: bool,
}

impl AppState {
    pub fn new(theme: Theme) -> Self {
        Self {
            screen: Screen::Runs,
            screen_stack: Vec::new(),
            theme,
            modal: None,
            dirty: true,
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
