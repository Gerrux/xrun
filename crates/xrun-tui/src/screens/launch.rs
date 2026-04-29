use crossterm::event::{KeyCode, KeyEvent};

use crate::state::AppState;

pub enum LaunchAction {
    Confirm(String),
    Back,
    Nothing,
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> LaunchAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => LaunchAction::Back,
        KeyCode::Up | KeyCode::Char('k') => {
            if state.launch.selected > 0 {
                state.launch.selected -= 1;
                state.dirty = true;
            }
            LaunchAction::Nothing
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let len = state.launch.manifests.len();
            if len > 0 && state.launch.selected < len - 1 {
                state.launch.selected += 1;
                state.dirty = true;
            }
            LaunchAction::Nothing
        }
        KeyCode::Enter => match state.launch.manifests.get(state.launch.selected) {
            Some(m) => LaunchAction::Confirm(m.path.to_string_lossy().to_string()),
            None => LaunchAction::Nothing,
        },
        _ => LaunchAction::Nothing,
    }
}
