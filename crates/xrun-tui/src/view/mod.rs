use ratatui::prelude::*;

use crate::state::{AppState, Screen};

mod status_bar;

pub fn render(f: &mut Frame, state: &AppState) {
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(f.area());

    render_main(f, chunks[0], state);
    status_bar::render(f, chunks[1], state);
}

fn render_main(f: &mut Frame, area: Rect, state: &AppState) {
    match &state.screen {
        Screen::Runs => {}
        Screen::RunDetail(_, _) => {}
        Screen::Launch => {}
        Screen::Instances => {}
        Screen::Settings => {}
    }
    let _ = (f, area);
}
