use crossterm::event::{KeyCode, KeyEvent};

use crate::state::AppState;

pub enum VendorsAction {
    Back,
    OpenEdit(String),
    ImportNative(String),
    TestConnection(String),
    ShowRevokeConfirm(String),
    Nothing,
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> VendorsAction {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            navigate(state, -1);
            VendorsAction::Nothing
        }
        KeyCode::Down | KeyCode::Char('j') => {
            navigate(state, 1);
            VendorsAction::Nothing
        }
        KeyCode::Esc | KeyCode::Char('q') => VendorsAction::Back,
        KeyCode::Char('e') | KeyCode::Enter => match selected(state) {
            Some(v) => VendorsAction::OpenEdit(v),
            None => VendorsAction::Nothing,
        },
        KeyCode::Char('i') => match selected(state) {
            Some(v) => VendorsAction::ImportNative(v),
            None => VendorsAction::Nothing,
        },
        KeyCode::Char('t') => match selected(state) {
            Some(v) => VendorsAction::TestConnection(v),
            None => VendorsAction::Nothing,
        },
        KeyCode::Char('r') => match selected(state) {
            Some(v) => VendorsAction::ShowRevokeConfirm(v),
            None => VendorsAction::Nothing,
        },
        _ => VendorsAction::Nothing,
    }
}

fn navigate(state: &mut AppState, delta: i32) {
    let len = state.vendors.vendors.len();
    if len == 0 {
        return;
    }
    if delta < 0 {
        state.vendors.selected = state.vendors.selected.saturating_sub((-delta) as usize);
    } else {
        state.vendors.selected = (state.vendors.selected + delta as usize).min(len - 1);
    }
    state.dirty = true;
}

fn selected(state: &AppState) -> Option<String> {
    state.vendors.vendors.get(state.vendors.selected).cloned()
}
