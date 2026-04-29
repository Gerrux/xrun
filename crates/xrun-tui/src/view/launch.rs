use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::state::AppState;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);

    render_list(f, chunks[0], state);
    render_preview(f, chunks[1], state);
}

fn render_list(f: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .launch
        .manifests
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let icon = if m.previously_run { "\u{2713} " } else { "  " };
            let style = if i == state.launch.selected {
                state.theme.selected
            } else {
                state.theme.normal
            };
            ListItem::new(format!("{}{}", icon, m.name)).style(style)
        })
        .collect();

    let block = Block::default()
        .title(" Launch \u{2014} exp/ ")
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    let list = List::new(items).block(block);
    let mut list_state = ListState::default();
    if !state.launch.manifests.is_empty() {
        list_state.select(Some(state.launch.selected));
    }
    f.render_stateful_widget(list, area, &mut list_state);
}

fn render_preview(f: &mut Frame, area: Rect, state: &AppState) {
    let content = state
        .launch
        .manifests
        .get(state.launch.selected)
        .map(|m| m.content.as_str())
        .unwrap_or("(no manifest selected)");

    let block = Block::default()
        .title(" Preview ")
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    f.render_widget(
        Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
