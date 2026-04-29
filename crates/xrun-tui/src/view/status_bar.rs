use chrono::Utc;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use xrun_core::budget;

use crate::state::{AppState, Screen};
use crate::view::SPINNER;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    // Three-panel layout: [left: xrun › breadcrumb] [center: vendor balance] [right: hotkeys]
    let chunks = Layout::horizontal([
        Constraint::Percentage(28),
        Constraint::Min(0),
        Constraint::Percentage(42),
    ])
    .split(area);

    render_left(f, chunks[0], state);
    render_center(f, chunks[1], state);
    render_right(f, chunks[2], state);
}

fn render_left(f: &mut Frame, area: Rect, state: &AppState) {
    let breadcrumb = screen_breadcrumb(state);
    let line = Line::from(vec![
        Span::styled(" xrun ", state.theme.accent),
        Span::styled("\u{203a} ", state.theme.dim_text),
        Span::styled(breadcrumb, state.theme.normal),
    ]);
    f.render_widget(Paragraph::new(line).style(state.theme.status_bar), area);
}

fn render_center(f: &mut Frame, area: Rect, state: &AppState) {
    let line = build_balance_line(state);
    f.render_widget(
        Paragraph::new(line)
            .style(state.theme.status_bar)
            .alignment(Alignment::Center),
        area,
    );
}

fn render_right(f: &mut Frame, area: Rect, state: &AppState) {
    let hotkeys = screen_hotkeys(&state.screen);
    f.render_widget(
        Paragraph::new(hotkeys)
            .style(state.theme.dim_text.patch(state.theme.status_bar))
            .alignment(Alignment::Right),
        area,
    );
}

fn build_balance_line(state: &AppState) -> Line<'static> {
    if state.credentials.is_empty() {
        return Line::from(vec![
            Span::styled(
                "no vendor configured \u{2014} press ",
                state.theme.status_bar,
            ),
            Span::styled("V", state.theme.accent),
        ]);
    }

    let preferred = state
        .default_vendor_name
        .clone()
        .or_else(|| first_configured_vendor(state));

    let Some(name) = preferred else {
        return Line::from(Span::styled("balance: \u{2014}", state.theme.status_bar));
    };

    match state.vendor_statuses.get(&name) {
        Some(s) if s.connected => {
            let icon = Span::styled("\u{25cf} ", state.theme.ok);
            let vendor = Span::styled(format!("{} ", name), state.theme.normal);
            match s.balance {
                Some(b) => {
                    let cur = s.currency.as_deref().unwrap_or("$");
                    let age = humanize_age_secs(
                        Utc::now()
                            .signed_duration_since(s.last_checked)
                            .num_seconds()
                            .max(0),
                    );
                    let burn = budget::active_hourly_burn_slice(&state.instances.instances);
                    let runway_warning = if burn > 0.0 && b / burn < 1.0 {
                        Some(Span::styled(
                            format!("  \u{26a0} <{:.0}m runway", (b / burn) * 60.0),
                            state.theme.failed,
                        ))
                    } else {
                        None
                    };
                    let mut spans = vec![
                        icon,
                        vendor,
                        Span::styled(format!("{}{:.2}", cur, b), state.theme.accent),
                        Span::styled(format!("  ({age})"), state.theme.dim_text),
                    ];
                    if let Some(w) = runway_warning {
                        spans.push(w);
                    }
                    Line::from(spans)
                }
                None => Line::from(vec![
                    icon,
                    vendor,
                    Span::styled("connected", state.theme.ok),
                ]),
            }
        }
        Some(s) => {
            let err = s.error.as_deref().unwrap_or("error");
            Line::from(vec![
                Span::styled("\u{25cf} ", state.theme.failed),
                Span::styled(format!("{}: {}", name, err), state.theme.normal),
            ])
        }
        None => {
            let spin = SPINNER[state.runs.throbber_frame as usize % SPINNER.len()];
            Line::from(Span::styled(
                format!("{} probing {}\u{2026}", spin, name),
                state.theme.running,
            ))
        }
    }
}

fn screen_breadcrumb(state: &AppState) -> String {
    let mut parts: Vec<&str> = state
        .screen_stack
        .iter()
        .map(|s| screen_short_name(s))
        .collect();
    parts.push(screen_short_name(&state.screen));
    parts.join(" \u{203a} ")
}

fn screen_short_name(screen: &Screen) -> &'static str {
    match screen {
        Screen::Runs => "runs",
        Screen::RunDetail(_, _) => "detail",
        Screen::Launch => "launch",
        Screen::Instances => "instances",
        Screen::Settings => "settings",
        Screen::Vendors => "vendors",
    }
}

fn screen_hotkeys(screen: &Screen) -> &'static str {
    match screen {
        Screen::Runs => "L:launch  S:stop  R:rerun  /:filter  ?:help  q:quit",
        Screen::RunDetail(_, _) => "Tab:tabs  e:editor  q:back  ?:help",
        Screen::Launch => "enter:launch  j/k:nav  Esc:back",
        Screen::Instances => "Tab:switch  D:destroy  j/k:nav  Esc:back",
        Screen::Settings => "j/k:nav  enter:edit  Esc:back",
        Screen::Vendors => "e:edit  i:import  t:test  r:revoke  Esc:back",
    }
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

fn humanize_age_secs(secs: i64) -> String {
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}
