use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use xrun_core::RunId;

use crate::state::{AppState, Modal, RunSection};

pub enum RunsAction {
    OpenRunDetail(RunId),
    OpenLaunch,
    OpenInstances,
    OpenSettings,
    ShowStopConfirm(RunId, String),
    ShowPullConfirm(RunId, String),
    Rerun(RunId),
    Quit,
    Nothing,
}

pub fn handle_key(state: &mut AppState, key: KeyEvent) -> RunsAction {
    if let Some(Modal::FilterInput { ref mut input }) = state.modal {
        match key.code {
            KeyCode::Esc => {
                state.modal = None;
                state.dirty = true;
            }
            KeyCode::Enter => {
                let filter = input.trim().to_string();
                state.modal = None;
                state.runs.filter = if filter.is_empty() {
                    None
                } else {
                    Some(filter)
                };
                state.runs.selected = 0;
                state.dirty = true;
            }
            KeyCode::Backspace => {
                input.pop();
                state.dirty = true;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.push(c);
                state.dirty = true;
            }
            _ => {}
        }
        return RunsAction::Nothing;
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            navigate(state, -1);
            RunsAction::Nothing
        }
        KeyCode::Down | KeyCode::Char('j') => {
            navigate(state, 1);
            RunsAction::Nothing
        }
        KeyCode::Tab => {
            toggle_section(state);
            RunsAction::Nothing
        }
        KeyCode::Enter => match selected_run_id(state) {
            Some(id) => RunsAction::OpenRunDetail(id),
            None => RunsAction::Nothing,
        },
        KeyCode::Char('L') => RunsAction::OpenLaunch,
        KeyCode::Char('I') => RunsAction::OpenInstances,
        KeyCode::Char(',') => RunsAction::OpenSettings,
        KeyCode::Char('S') => match selected_active_run(state) {
            Some((id, name)) => RunsAction::ShowStopConfirm(id, name),
            None => RunsAction::Nothing,
        },
        KeyCode::Char('P') => match selected_recent_run(state) {
            Some((id, name)) => RunsAction::ShowPullConfirm(id, name),
            None => RunsAction::Nothing,
        },
        KeyCode::Char('R') => match selected_run_id(state) {
            Some(id) => RunsAction::Rerun(id),
            None => RunsAction::Nothing,
        },
        KeyCode::Char('/') => {
            state.modal = Some(Modal::FilterInput {
                input: String::new(),
            });
            state.dirty = true;
            RunsAction::Nothing
        }
        KeyCode::Char('q') | KeyCode::Esc => RunsAction::Quit,
        _ => RunsAction::Nothing,
    }
}

fn navigate(state: &mut AppState, delta: i32) {
    let len = match state.runs.section {
        RunSection::Active => state.runs.active_runs.len(),
        RunSection::Recent => state.runs.recent_runs.len(),
    };
    if len == 0 {
        return;
    }
    if delta < 0 {
        state.runs.selected = state.runs.selected.saturating_sub((-delta) as usize);
    } else {
        state.runs.selected = (state.runs.selected + delta as usize).min(len - 1);
    }
    state.dirty = true;
}

fn toggle_section(state: &mut AppState) {
    state.runs.section = match state.runs.section {
        RunSection::Active => RunSection::Recent,
        RunSection::Recent => RunSection::Active,
    };
    state.runs.selected = 0;
    state.dirty = true;
}

fn selected_run_id(state: &AppState) -> Option<RunId> {
    match state.runs.section {
        RunSection::Active => state
            .runs
            .active_runs
            .get(state.runs.selected)
            .map(|r| r.id.clone()),
        RunSection::Recent => state
            .runs
            .recent_runs
            .get(state.runs.selected)
            .map(|r| r.id.clone()),
    }
}

fn selected_active_run(state: &AppState) -> Option<(RunId, String)> {
    if state.runs.section == RunSection::Active {
        state
            .runs
            .active_runs
            .get(state.runs.selected)
            .map(|r| (r.id.clone(), r.name.clone()))
    } else {
        None
    }
}

fn selected_recent_run(state: &AppState) -> Option<(RunId, String)> {
    if state.runs.section == RunSection::Recent {
        state
            .runs
            .recent_runs
            .get(state.runs.selected)
            .map(|r| (r.id.clone(), r.name.clone()))
    } else {
        None
    }
}
