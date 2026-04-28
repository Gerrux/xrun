use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::state::AppState;

const LABELS: &[&str] = &[
    "theme",
    "poll_interval_active (s)",
    "poll_interval_idle (s)",
    "default_vendor",
];

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);

    render_table(f, chunks[0], state);
    render_edit_area(f, chunks[1], state);
    render_hints(f, chunks[2], state);
}

fn render_table(f: &mut Frame, area: Rect, state: &AppState) {
    let values = [
        state.settings.theme.as_str().to_string(),
        state.settings.poll_interval_active.to_string(),
        state.settings.poll_interval_idle.to_string(),
        if state.settings.default_vendor.is_empty() {
            "\u{2014}".to_string()
        } else {
            state.settings.default_vendor.clone()
        },
    ];

    let rows: Vec<Row> = LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let row_style = if i == state.settings.selected_row {
                state.theme.selected
            } else {
                state.theme.normal
            };
            Row::new(vec![Cell::from(*label), Cell::from(values[i].clone())]).style(row_style)
        })
        .collect();

    let widths = [Constraint::Percentage(40), Constraint::Min(20)];

    let header = Row::new(["Setting", "Value"]).style(state.theme.title);

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" Settings ")
                .borders(Borders::ALL)
                .border_style(state.theme.border),
        )
        .row_highlight_style(state.theme.selected);

    let mut tbl_state = TableState::default();
    tbl_state.select(Some(state.settings.selected_row));

    f.render_stateful_widget(table, area, &mut tbl_state);
}

fn render_edit_area(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(if state.settings.editing {
            " Edit \u{2014} Enter=save  Esc=cancel "
        } else {
            " Value \u{2014} Enter/e=edit "
        })
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    let content = if state.settings.editing {
        format!("> {}_", state.settings.edit_input)
    } else {
        let values = [
            state.settings.theme.as_str().to_string(),
            state.settings.poll_interval_active.to_string(),
            state.settings.poll_interval_idle.to_string(),
            state.settings.default_vendor.clone(),
        ];
        values
            .get(state.settings.selected_row)
            .cloned()
            .unwrap_or_default()
    };

    f.render_widget(Paragraph::new(content).block(block), area);
}

fn render_hints(f: &mut Frame, area: Rect, state: &AppState) {
    let hint = if state.settings.editing {
        "Enter:save  Esc:cancel"
    } else {
        "j/k:navigate  Enter/e:edit  Esc/q:back  (credentials: edit ~/.config/xrun/credentials.toml)"
    };
    f.render_widget(Paragraph::new(hint).style(state.theme.status_bar), area);
}
