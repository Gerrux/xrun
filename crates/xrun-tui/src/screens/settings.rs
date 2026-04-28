use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::state::AppState;

pub(crate) const SETTINGS_ROW_COUNT: usize = 4;

pub enum SettingsAction {
    SaveTheme(String),
    SavePollIntervalActive(u64),
    SavePollIntervalIdle(u64),
    SaveDefaultVendor(Option<String>),
    Back,
    Nothing,
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> SettingsAction {
    if state.settings.editing {
        return handle_edit_key(state, key);
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => SettingsAction::Back,
        KeyCode::Up | KeyCode::Char('k') => {
            if state.settings.selected_row > 0 {
                state.settings.selected_row -= 1;
                state.dirty = true;
            }
            SettingsAction::Nothing
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.settings.selected_row < SETTINGS_ROW_COUNT - 1 {
                state.settings.selected_row += 1;
                state.dirty = true;
            }
            SettingsAction::Nothing
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            state.settings.edit_input = current_value(state).to_string();
            state.settings.editing = true;
            state.dirty = true;
            SettingsAction::Nothing
        }
        _ => SettingsAction::Nothing,
    }
}

fn handle_edit_key(state: &mut AppState, key: KeyEvent) -> SettingsAction {
    match key.code {
        KeyCode::Esc => {
            state.settings.editing = false;
            state.settings.edit_input.clear();
            state.dirty = true;
            SettingsAction::Nothing
        }
        KeyCode::Enter => {
            let value = state.settings.edit_input.trim().to_string();
            state.settings.editing = false;
            state.settings.edit_input.clear();
            state.dirty = true;
            apply_value(state.settings.selected_row, value)
        }
        KeyCode::Backspace => {
            state.settings.edit_input.pop();
            state.dirty = true;
            SettingsAction::Nothing
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.settings.edit_input.push(c);
            state.dirty = true;
            SettingsAction::Nothing
        }
        _ => SettingsAction::Nothing,
    }
}

fn current_value(state: &AppState) -> &str {
    match state.settings.selected_row {
        0 => &state.settings.theme,
        1 | 2 => "",
        3 => &state.settings.default_vendor,
        _ => "",
    }
}

fn apply_value(row: usize, value: String) -> SettingsAction {
    match row {
        0 => SettingsAction::SaveTheme(value),
        1 => match value.parse::<u64>() {
            Ok(v) => SettingsAction::SavePollIntervalActive(v),
            Err(_) => SettingsAction::Nothing,
        },
        2 => match value.parse::<u64>() {
            Ok(v) => SettingsAction::SavePollIntervalIdle(v),
            Err(_) => SettingsAction::Nothing,
        },
        3 => SettingsAction::SaveDefaultVendor(if value.is_empty() { None } else { Some(value) }),
        _ => SettingsAction::Nothing,
    }
}
