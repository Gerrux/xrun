use chrono::Utc;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use xrun_core::RunStatus;

use crate::state::AppState;
use crate::view::SPINNER;

pub(super) fn render_header_cards(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }
    let chunks = Layout::horizontal([
        Constraint::Percentage(33),
        Constraint::Percentage(34),
        Constraint::Percentage(33),
    ])
    .split(area);

    render_vendor_card(f, chunks[0], state);
    render_active_card(f, chunks[1], state);
    render_today_card(f, chunks[2], state);
}

fn card_block<'a>(label: &'a str, state: &AppState) -> Block<'a> {
    Block::default()
        .title(Span::styled(label, state.theme.dim_text))
        .borders(Borders::ALL)
        .border_style(state.theme.accent)
}

fn render_vendor_card(f: &mut Frame, area: Rect, state: &AppState) {
    let block = card_block(" vendor ", state);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.credentials.is_empty() {
        let line = Line::from(vec![
            Span::styled("Configure \u{2192} press ", state.theme.dim_text),
            Span::styled("V", state.theme.accent),
        ]);
        f.render_widget(Paragraph::new(line).alignment(Alignment::Center), inner);
        return;
    }

    let preferred = state
        .default_vendor_name
        .clone()
        .or_else(|| first_configured_vendor(state));

    let Some(name) = preferred else {
        f.render_widget(
            Paragraph::new(Span::styled("no vendor", state.theme.dim_text))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    };

    let status = state.vendor_statuses.get(&name);
    let (icon, icon_style) = match status {
        Some(s) if s.connected => ("\u{25cf}", state.theme.ok),
        Some(_) => ("\u{25cf}", state.theme.failed),
        None => {
            let spin = SPINNER[state.runs.throbber_frame as usize % SPINNER.len()];
            (spin, state.theme.running)
        }
    };

    let value_text = match status {
        Some(s) if s.connected => match s.balance {
            Some(b) => {
                let cur = s.currency.as_deref().unwrap_or("$");
                format!("{}{:.2}", cur, b)
            }
            None => "connected".to_string(),
        },
        Some(s) => {
            let e = s.error.as_deref().unwrap_or("error");
            e.chars().take(14).collect()
        }
        None => "probing\u{2026}".to_string(),
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(name, state.theme.normal),
        ]),
        Line::from(Span::styled(value_text, state.theme.accent)),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn render_active_card(f: &mut Frame, area: Rect, state: &AppState) {
    let block = card_block(" active ", state);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let total = state.runs.active_runs.len();
    let provisioning = state
        .runs
        .active_runs
        .iter()
        .filter(|r| r.status == RunStatus::Provisioning)
        .count();
    let uploading = state
        .runs
        .active_runs
        .iter()
        .filter(|r| r.status == RunStatus::Uploading)
        .count();
    let running = state
        .runs
        .active_runs
        .iter()
        .filter(|r| r.status == RunStatus::Running)
        .count();

    let main_line = if total == 0 {
        Line::from(Span::styled("none", state.theme.dim_text))
    } else {
        Line::from(Span::styled(
            format!("{} running", total),
            state.theme.accent,
        ))
    };

    let mut detail_parts: Vec<String> = Vec::new();
    if running > 0 && (uploading > 0 || provisioning > 0) {
        detail_parts.push(format!("{} active", running));
    }
    if uploading > 0 {
        detail_parts.push(format!("{} upload", uploading));
    }
    if provisioning > 0 {
        detail_parts.push(format!("{} prov", provisioning));
    }

    let detail_line = if detail_parts.is_empty() {
        Line::from("")
    } else {
        Line::from(Span::styled(
            detail_parts.join(" \u{00b7} "),
            state.theme.dim_text,
        ))
    };

    f.render_widget(
        Paragraph::new(vec![main_line, detail_line]).alignment(Alignment::Center),
        inner,
    );
}

fn render_today_card(f: &mut Frame, area: Rect, state: &AppState) {
    let block = card_block(" today ", state);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let now = Utc::now();
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc())
        .unwrap_or(now);

    let today_runs: Vec<_> = state
        .runs
        .recent_runs
        .iter()
        .filter(|r| r.ended_at.map(|e| e >= today_start).unwrap_or(false))
        .collect();

    let lines = if today_runs.is_empty() {
        vec![Line::from(Span::styled(
            "no runs today",
            state.theme.dim_text,
        ))]
    } else {
        let done = today_runs
            .iter()
            .filter(|r| r.status == RunStatus::Done)
            .count();
        let failed = today_runs
            .iter()
            .filter(|r| r.status == RunStatus::Failed)
            .count();
        let cost: f64 = today_runs.iter().filter_map(|r| r.cost_usd).sum();

        let mut summary = format!("{} done", done);
        if failed > 0 {
            summary.push_str(&format!(" \u{00b7} {} failed", failed));
        }

        vec![
            Line::from(Span::styled(summary, state.theme.normal)),
            Line::from(Span::styled(
                format!("${:.2} spent", cost),
                state.theme.dim_text,
            )),
        ]
    };

    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn first_configured_vendor(state: &AppState) -> Option<String> {
    if state.credentials.vast.api_key.is_some() {
        return Some("vast".to_string());
    }
    if state.credentials.kaggle.username.is_some() && state.credentials.kaggle.key.is_some() {
        return Some("kaggle".to_string());
    }
    if state.credentials.mlflow.token.is_some() {
        return Some("mlflow".to_string());
    }
    None
}
