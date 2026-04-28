use crossterm::event::{KeyCode, KeyEvent};

use crate::state::AppState;

pub enum InstancesAction {
    ShowDestroyConfirm(String),
    Back,
    Nothing,
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> InstancesAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => InstancesAction::Back,
        KeyCode::Up | KeyCode::Char('k') => {
            if state.instances.selected > 0 {
                state.instances.selected -= 1;
                state.dirty = true;
            }
            InstancesAction::Nothing
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let len = state.instances.instances.len();
            if len > 0 && state.instances.selected < len - 1 {
                state.instances.selected += 1;
                state.dirty = true;
            }
            InstancesAction::Nothing
        }
        KeyCode::Char('D') => {
            if let Some(inst) = state.instances.instances.get(state.instances.selected) {
                if inst.run_id.is_none() && inst.destroyed_at.is_none() {
                    InstancesAction::ShowDestroyConfirm(inst.id.clone())
                } else {
                    InstancesAction::Nothing
                }
            } else {
                InstancesAction::Nothing
            }
        }
        _ => InstancesAction::Nothing,
    }
}
