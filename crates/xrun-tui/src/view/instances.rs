use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::state::AppState;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    render_table(f, chunks[0], state);
    render_hints(f, chunks[1], state);
}

fn render_table(f: &mut Frame, area: Rect, state: &AppState) {
    let header = Row::new(["GPU", "Vendor", "Price/h", "Uptime", "Run", "Status"])
        .style(state.theme.title)
        .bottom_margin(0);

    let rows: Vec<Row> = state
        .instances
        .instances
        .iter()
        .enumerate()
        .map(|(i, inst)| {
            let row_style = if i == state.instances.selected {
                state.theme.selected
            } else {
                state.theme.normal
            };

            let gpu = inst.gpu_type.as_deref().unwrap_or("\u{2014}").to_string();
            let price = inst
                .price_per_hour
                .map(|p| format!("${:.2}", p))
                .unwrap_or_else(|| "\u{2014}".to_string());
            let run = inst.run_id.as_deref().unwrap_or("orphan").to_string();
            let status = if inst.destroyed_at.is_some() {
                "destroyed"
            } else {
                "active"
            };
            let uptime = inst
                .created_at
                .map(|c| {
                    let secs = chrono::Utc::now()
                        .signed_duration_since(c)
                        .num_seconds()
                        .max(0);
                    if secs < 3600 {
                        format!("{}m{}s", secs / 60, secs % 60)
                    } else {
                        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
                    }
                })
                .unwrap_or_else(|| "\u{2014}".to_string());

            Row::new(vec![
                Cell::from(gpu),
                Cell::from(inst.vendor.clone()),
                Cell::from(price),
                Cell::from(uptime),
                Cell::from(run),
                Cell::from(status),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Min(12),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(9),
        Constraint::Min(10),
        Constraint::Length(9),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Instances ({}) ", state.instances.instances.len()))
                .borders(Borders::ALL)
                .border_style(state.theme.border),
        )
        .row_highlight_style(state.theme.selected);

    let mut tbl_state = TableState::default();
    if !state.instances.instances.is_empty() {
        tbl_state.select(Some(
            state
                .instances
                .selected
                .min(state.instances.instances.len() - 1),
        ));
    }

    f.render_stateful_widget(table, area, &mut tbl_state);
}

fn render_hints(f: &mut Frame, area: Rect, state: &AppState) {
    f.render_widget(
        Paragraph::new("D:destroy-orphan  Esc/q:back").style(state.theme.status_bar),
        area,
    );
}
