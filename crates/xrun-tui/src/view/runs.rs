use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use xrun_core::{Run, RunStatus};

use crate::state::{AppState, RunSection};

use super::SPINNER;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::vertical([
        Constraint::Percentage(50),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_active(f, chunks[0], state);
    render_recent(f, chunks[1], state);
    render_hints(f, chunks[2], state);
}

fn format_uptime(run: &Run) -> String {
    match run.started_at {
        None => "\u{2014}".to_string(),
        Some(t) => {
            let d = chrono::Utc::now().signed_duration_since(t);
            let secs = d.num_seconds().max(0);
            if secs < 60 {
                format!("{}s", secs)
            } else if secs < 3600 {
                format!("{}m{}s", secs / 60, secs % 60)
            } else {
                format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
            }
        }
    }
}

fn apply_filter<'a>(runs: &'a [Run], filter: Option<&String>) -> Vec<&'a Run> {
    match filter {
        None => runs.iter().collect(),
        Some(f) => {
            let fl = f.to_lowercase();
            runs.iter()
                .filter(|r| r.name.to_lowercase().contains(&fl))
                .collect()
        }
    }
}

fn render_active(f: &mut Frame, area: Rect, state: &AppState) {
    let is_focused = state.runs.section == RunSection::Active;
    let border_style = if is_focused {
        Style::default().fg(Color::White)
    } else {
        state.theme.border
    };

    let filtered = apply_filter(&state.runs.active_runs, state.runs.filter.as_ref());
    let spin_ch = SPINNER[state.runs.throbber_frame as usize % SPINNER.len()];

    let header = Row::new(["", "Name", "Vendor", "Uptime", "Status"])
        .style(state.theme.title)
        .bottom_margin(0);

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let row_style = if is_focused && i == state.runs.selected {
                state.theme.selected
            } else {
                state.theme.normal
            };
            Row::new(vec![
                Cell::from(spin_ch).style(state.theme.running),
                Cell::from(run.name.as_str()),
                Cell::from(run.vendor.as_str()),
                Cell::from(format_uptime(run)),
                Cell::from(run.status.as_str()).style(state.theme.running),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Min(14),
        Constraint::Length(8),
        Constraint::Length(9),
        Constraint::Length(13),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Active ({}) ", filtered.len()))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(state.theme.selected);

    let mut tbl_state = TableState::default();
    if is_focused && !filtered.is_empty() {
        tbl_state.select(Some(
            state.runs.selected.min(filtered.len().saturating_sub(1)),
        ));
    }

    f.render_stateful_widget(table, area, &mut tbl_state);
}

fn render_recent(f: &mut Frame, area: Rect, state: &AppState) {
    let is_focused = state.runs.section == RunSection::Recent;
    let border_style = if is_focused {
        Style::default().fg(Color::White)
    } else {
        state.theme.border
    };

    let filtered = apply_filter(&state.runs.recent_runs, state.runs.filter.as_ref());

    let header = Row::new(["Status", "Name", "Vendor", "Duration", "Cost"])
        .style(state.theme.title)
        .bottom_margin(0);

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let status_style = match &run.status {
                RunStatus::Done => state.theme.ok,
                RunStatus::Failed => state.theme.failed,
                RunStatus::Cancelled => state.theme.warn,
                _ => state.theme.normal,
            };

            let duration = match (run.started_at, run.ended_at) {
                (Some(s), Some(e)) => {
                    let secs = e.signed_duration_since(s).num_seconds().max(0);
                    if secs < 3600 {
                        format!("{}m{}s", secs / 60, secs % 60)
                    } else {
                        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
                    }
                }
                _ => "\u{2014}".to_string(),
            };

            let cost = run
                .cost_usd
                .map(|c| format!("${:.2}", c))
                .unwrap_or_else(|| "\u{2014}".to_string());

            let row_style = if is_focused && i == state.runs.selected {
                state.theme.selected
            } else {
                state.theme.normal
            };

            Row::new(vec![
                Cell::from(run.status.as_str()).style(status_style),
                Cell::from(run.name.as_str()),
                Cell::from(run.vendor.as_str()),
                Cell::from(duration),
                Cell::from(cost),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(11),
        Constraint::Min(14),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Recent ({}) ", filtered.len()))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(state.theme.selected);

    let mut tbl_state = TableState::default();
    if is_focused && !filtered.is_empty() {
        tbl_state.select(Some(
            state.runs.selected.min(filtered.len().saturating_sub(1)),
        ));
    }

    f.render_stateful_widget(table, area, &mut tbl_state);
}

fn render_hints(f: &mut Frame, area: Rect, state: &AppState) {
    let filter_hint = state
        .runs
        .filter
        .as_ref()
        .map(|f| format!("  filter:{}", f))
        .unwrap_or_default();
    let hint = format!(
        "enter:open  L:launch  S:stop  P:pull  R:rerun  /:filter  T:tags{}",
        filter_hint
    );
    f.render_widget(Paragraph::new(hint).style(state.theme.status_bar), area);
}
