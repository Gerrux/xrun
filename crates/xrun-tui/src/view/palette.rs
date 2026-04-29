use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::state::AppState;

pub fn render(
    f: &mut Frame,
    state: &AppState,
    input: &str,
    completions: &[String],
    selected: usize,
) {
    let area = super::centered_rect(62, 50, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Command  Tab=autocomplete  Enter=run  Esc=cancel ")
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);

    f.render_widget(Paragraph::new(format!(": {}_", input)), chunks[0]);

    if !completions.is_empty() {
        let items: Vec<ListItem> = completions
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let style = if i == selected {
                    state.theme.selected
                } else {
                    state.theme.normal
                };
                ListItem::new(c.as_str()).style(style)
            })
            .collect();
        f.render_widget(List::new(items), chunks[1]);
    }
}
