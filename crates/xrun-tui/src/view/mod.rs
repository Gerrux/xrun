use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::state::{AppState, Modal, Screen};

mod run_detail;
mod runs;
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
        Screen::Launch => {}
        Screen::Instances => {}
        Screen::Settings => {}
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
            let area = centered_rect(70, 80, f.area());
            f.render_widget(Clear, area);
            let block = Block::default()
                .title(" Help \u{2014} any key to close ")
                .borders(Borders::ALL)
                .border_style(state.theme.border);
            let help = "Global\n\
                         \n\
                         q / Esc   quit / back\n\
                         ?         this help\n\
                         :         command palette\n\
                         \n\
                         Runs screen\n\
                         \n\
                         enter     open run detail\n\
                         L         launch picker\n\
                         S         stop run\n\
                         P         pull checkpoint\n\
                         R         rerun\n\
                         /         filter\n\
                         Tab       switch section";
            f.render_widget(
                Paragraph::new(help).block(block).wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
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
