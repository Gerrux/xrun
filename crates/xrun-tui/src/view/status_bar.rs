use ratatui::{prelude::*, widgets::Paragraph};

use crate::state::AppState;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let text = " xrun  \u{2022}  balance: $\u{2014}  \u{2022}  q quit  \u{2022}  ? help ";
    let paragraph = Paragraph::new(text).style(state.theme.status_bar);
    f.render_widget(paragraph, area);
}
