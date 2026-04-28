use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::state::AppState;

pub struct EmptyState<'a> {
    pub icon: &'a str,
    pub title: &'a str,
    pub hint: &'a str,
}

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState, es: EmptyState<'_>) {
    if area.height == 0 {
        return;
    }
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(es.icon, state.theme.dim_text)),
        Line::from(""),
        Line::from(Span::styled(es.title, state.theme.normal)),
        Line::from(""),
        Line::from(Span::styled(es.hint, state.theme.dim_text)),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}
