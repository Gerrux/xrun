use crossterm::event::{KeyCode, KeyEvent};

use crate::state::{AppState, InstancesSection};

pub enum InstancesAction {
    ShowDestroyConfirm(String),
    Back,
    Nothing,
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> InstancesAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => InstancesAction::Back,
        KeyCode::Tab => {
            state.instances.section = match state.instances.section {
                InstancesSection::Local => InstancesSection::Remote,
                InstancesSection::Remote => InstancesSection::Local,
            };
            state.instances.selected = 0;
            state.dirty = true;
            InstancesAction::Nothing
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.instances.selected > 0 {
                state.instances.selected -= 1;
                state.dirty = true;
            }
            InstancesAction::Nothing
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let len = section_len(state);
            if len > 0 && state.instances.selected < len - 1 {
                state.instances.selected += 1;
                state.dirty = true;
            }
            InstancesAction::Nothing
        }
        KeyCode::Char('D') => {
            if state.instances.section == InstancesSection::Local {
                if let Some(inst) = state.instances.instances.get(state.instances.selected) {
                    if inst.run_id.is_none() && inst.destroyed_at.is_none() {
                        return InstancesAction::ShowDestroyConfirm(inst.id.clone());
                    }
                }
            }
            InstancesAction::Nothing
        }
        _ => InstancesAction::Nothing,
    }
}

fn section_len(state: &AppState) -> usize {
    match state.instances.section {
        InstancesSection::Local => state.instances.instances.len(),
        InstancesSection::Remote => state.instances.remote.len(),
    }
}
