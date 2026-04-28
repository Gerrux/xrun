use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use xrun_core::{Run, RunStatus};

use crate::state::{AppState, RunSection};

use super::anim;
use super::empty::EmptyState;
use super::SPINNER;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let all_empty = state.runs.active_runs.is_empty() && state.runs.recent_runs.is_empty();

    let card_height = 5u16.min(area.height);
    let remaining_height = area.height.saturating_sub(card_height);

    // Adaptive active section height: collapsed to 3 when empty, else (N+3).clamp(5, h/2)
    let active_height: u16 = if state.runs.active_runs.is_empty() {
        3
    } else {
        (state.runs.active_runs.len() as u16 + 3).clamp(5, remaining_height / 2)
    };

    if all_empty {
        let chunks =
            Layout::vertical([Constraint::Length(card_height), Constraint::Min(0)]).split(area);
        super::cards::render_header_cards(f, chunks[0], state);
        super::empty::render(
            f,
            chunks[1],
            state,
            EmptyState {
                icon: "\u{29b8}",
                title: "No runs yet",
                hint: "press L to launch",
            },
        );
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(card_height),
        Constraint::Length(active_height),
        Constraint::Min(0),
    ])
    .split(area);

    super::cards::render_header_cards(f, chunks[0], state);
    render_active(f, chunks[1], state);
    render_recent(f, chunks[2], state);
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

    // Empty state inside the section
    if filtered.is_empty() {
        let block = Block::default()
            .title(format!(" Active ({}) ", state.runs.active_runs.len()))
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = if state.runs.active_runs.is_empty() {
            "no active runs"
        } else {
            "no runs match filter"
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, state.theme.dim_text)).alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let header = Row::new(["", "Name", "Vendor", "Uptime", "Status"])
        .style(state.theme.title)
        .bottom_margin(0);

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let selected = is_focused && i == state.runs.selected;
            let row_style = if selected {
                anim::pulse(state.anim_frame, state.theme.selected)
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

    // Empty state inside the section
    if filtered.is_empty() {
        let block = Block::default()
            .title(format!(" Recent ({}) ", state.runs.recent_runs.len()))
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = if state.runs.recent_runs.is_empty() {
            "no recent runs"
        } else {
            "no runs match filter"
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, state.theme.dim_text)).alignment(Alignment::Center),
            inner,
        );
        return;
    }

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

            let selected = is_focused && i == state.runs.selected;
            let row_style = if selected {
                anim::pulse(state.anim_frame, state.theme.selected)
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
