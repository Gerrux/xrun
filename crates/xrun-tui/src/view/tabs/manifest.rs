use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::state::AppState;

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let text = if state.run_detail.manifest_text.is_empty() {
        "(manifest not found)".to_string()
    } else {
        state.run_detail.manifest_text.clone()
    };

    let block = Block::default()
        .title(" Manifest  e:edit ")
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    f.render_widget(
        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
        area,
    );
}
