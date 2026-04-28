use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;

use crate::state::{AppState, Screen, Tab};

pub enum RunDetailAction {
    SwitchTab(Tab),
    Back,
    OpenEditor(PathBuf),
    ToggleAutoscroll,
    ScrollUp,
    ScrollDown,
    ScrollTop,
    ScrollBottom,
    Nothing,
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> RunDetailAction {
    let current_tab = match &state.screen {
        Screen::RunDetail(_, tab) => tab.clone(),
        _ => return RunDetailAction::Nothing,
    };

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => RunDetailAction::Back,

        KeyCode::Tab => {
            let next = match current_tab {
                Tab::Stages => Tab::Logs,
                Tab::Logs => Tab::Manifest,
                Tab::Manifest => Tab::Stages,
            };
            RunDetailAction::SwitchTab(next)
        }
        KeyCode::BackTab => {
            let prev = match current_tab {
                Tab::Stages => Tab::Manifest,
                Tab::Logs => Tab::Stages,
                Tab::Manifest => Tab::Logs,
            };
            RunDetailAction::SwitchTab(prev)
        }

        KeyCode::Char('e') if current_tab == Tab::Manifest => {
            let path = state
                .run_detail
                .run
                .as_ref()
                .map(|r| PathBuf::from(&r.manifest_path));
            match path {
                Some(p) if p.exists() => RunDetailAction::OpenEditor(p),
                _ => RunDetailAction::Nothing,
            }
        }

        KeyCode::Char(' ') if current_tab == Tab::Logs => RunDetailAction::ToggleAutoscroll,
        KeyCode::Up | KeyCode::Char('k') if current_tab == Tab::Logs => RunDetailAction::ScrollUp,
        KeyCode::Down | KeyCode::Char('j') if current_tab == Tab::Logs => {
            RunDetailAction::ScrollDown
        }
        KeyCode::Home if current_tab == Tab::Logs => RunDetailAction::ScrollTop,
        KeyCode::End if current_tab == Tab::Logs => RunDetailAction::ScrollBottom,

        _ => RunDetailAction::Nothing,
    }
}
