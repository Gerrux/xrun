use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(crate) const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

use crate::state::{AppState, Modal, Screen};

mod help;
mod instances;
mod launch;
mod palette;
mod run_detail;
mod runs;
mod settings;
mod status_bar;
pub mod tabs;

pub fn render(f: &mut Frame, state: &AppState) {
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(f.area());

    render_main(f, chunks[0], state);
    status_bar::render(f, chunks[1], state);

    if state.modal.is_some() {
        render_modal(f, state);
    }
}

fn render_main(f: &mut Frame, area: Rect, state: &AppState) {
    match &state.screen {
        Screen::Runs => runs::render(f, area, state),
        Screen::RunDetail(_, _) => run_detail::render(f, area, state),
        Screen::Launch => launch::render(f, area, state),
        Screen::Instances => instances::render(f, area, state),
        Screen::Settings => settings::render(f, area, state),
    }
}

fn render_modal(f: &mut Frame, state: &AppState) {
    let Some(modal) = &state.modal else { return };
    match modal {
        Modal::Confirm { message, .. } => {
            let area = centered_rect(50, 30, f.area());
            f.render_widget(Clear, area);
            let block = Block::default()
                .title(" Confirm ")
                .borders(Borders::ALL)
                .border_style(state.theme.border);
            let text = format!("{}\n\n  y = yes   n / Esc = no", message);
            f.render_widget(
                Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                area,
            );
        }
        Modal::FilterInput { input } => {
            let area = centered_rect(60, 15, f.area());
            f.render_widget(Clear, area);
            let block = Block::default()
                .title(" Filter  Enter=apply  Esc=cancel ")
                .borders(Borders::ALL)
                .border_style(state.theme.border);
            f.render_widget(Paragraph::new(format!("> {}_", input)).block(block), area);
        }
        Modal::Help => {
            help::render(f, state);
        }
        Modal::CommandPalette {
            input,
            completions,
            selected_completion,
        } => {
            palette::render(f, state, input, completions, *selected_completion);
        }
    }
}

pub(super) fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vert = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vert[1])[1]
}
