use chrono::Utc;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use xrun_core::vendor::VendorStatus;

use crate::state::AppState;

use super::anim;
use super::empty::EmptyState;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).split(area);

    render_table(f, chunks[0], state);
    render_detail(f, chunks[1], state);
}

fn render_table(f: &mut Frame, area: Rect, state: &AppState) {
    let header = Row::new(["Vendor", "Status", "Account", "Balance", "Last checked"])
        .style(state.theme.title);

    let rows: Vec<Row> = state
        .vendors
        .vendors
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let configured = is_configured(state, v);
            let status_opt = state.vendor_statuses.get(v);
            let (status_label, status_style) = render_status_cell(state, configured, status_opt);
            let account = status_opt
                .and_then(|s| s.account.clone())
                .unwrap_or_else(|| {
                    if configured {
                        "\u{2014}".to_string()
                    } else {
                        String::new()
                    }
                });
            let balance = format_balance(status_opt);
            let last = format_last_checked(status_opt);
            let selected = i == state.vendors.selected;
            let row_style = if selected {
                anim::pulse(state.anim_frame, state.theme.selected)
            } else {
                state.theme.normal
            };
            Row::new(vec![
                Cell::from(v.clone()),
                Cell::from(status_label).style(status_style),
                Cell::from(account),
                Cell::from(balance),
                Cell::from(last),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(20),
        Constraint::Min(18),
        Constraint::Length(12),
        Constraint::Length(14),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" Vendors ")
                .borders(Borders::ALL)
                .border_style(state.theme.border),
        )
        .row_highlight_style(state.theme.selected);

    let mut tbl_state = TableState::default();
    if !state.vendors.vendors.is_empty() {
        tbl_state.select(Some(
            state
                .vendors
                .selected
                .min(state.vendors.vendors.len().saturating_sub(1)),
        ));
    }
    f.render_stateful_widget(table, area, &mut tbl_state);
}

fn render_detail(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    let Some(vendor) = state.vendors.vendors.get(state.vendors.selected) else {
        f.render_widget(Paragraph::new("(no vendor selected)").block(block), area);
        return;
    };

    let configured = is_configured(state, vendor);

    // Not configured: show dedicated empty-state with shortcut hints
    if !configured {
        let inner = block.inner(area);
        f.render_widget(block, area);

        let hint = format!(
            "e: enter creds  i: import native  ({} not configured)",
            vendor
        );
        super::empty::render(
            f,
            inner,
            state,
            EmptyState {
                icon: "\u{2717}",
                title: "Not configured",
                hint: &hint,
            },
        );

        // Show native path note if applicable
        let path_note: Option<String> = if vendor == "vast" {
            xrun_core::config::Credentials::vast_native_path()
                .map(|p| format!("native key: {}", p.display()))
        } else if vendor == "kaggle" {
            xrun_core::config::Credentials::kaggle_native_path()
                .map(|p| format!("native key: {}", p.display()))
        } else {
            None
        };

        if let Some(note) = path_note {
            if area.height > 8 {
                // Overlay the path note at the bottom of the inner area
                let note_area = Rect {
                    x: inner.x + 1,
                    y: inner.y + inner.height.saturating_sub(2),
                    width: inner.width.saturating_sub(2),
                    height: 1,
                };
                f.render_widget(
                    Paragraph::new(Span::styled(note, state.theme.dim_text)),
                    note_area,
                );
            }
        }
        return;
    }

    let status = state.vendor_statuses.get(vendor);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("{:>10}: ", "vendor"), state.theme.title),
        Span::raw(vendor.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("{:>10}: ", "configured"), state.theme.title),
        Span::raw("yes"),
    ]));

    if let Some(s) = status {
        lines.push(Line::from(vec![
            Span::styled(format!("{:>10}: ", "connected"), state.theme.title),
            Span::raw(if s.connected { "yes" } else { "no" }),
        ]));
        if let Some(acc) = &s.account {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>10}: ", "account"), state.theme.title),
                Span::raw(acc.clone()),
            ]));
        }
        if let Some(b) = s.balance {
            let cur = s.currency.as_deref().unwrap_or("USD");
            lines.push(Line::from(vec![
                Span::styled(format!("{:>10}: ", "balance"), state.theme.title),
                Span::styled(format!("{} {:.2}", cur, b), state.theme.accent),
            ]));
        }
        if let Some(err) = &s.error {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>10}: ", "error"), state.theme.title),
                Span::styled(err.clone(), state.theme.failed),
            ]));
        }
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Configured. Press 't' to test connection.",
            state.theme.normal,
        )));
    }

    if let Some(flash) = &state.vendors.flash {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(flash.clone(), state.theme.warn)));
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_status_cell(
    state: &AppState,
    configured: bool,
    status: Option<&VendorStatus>,
) -> (String, Style) {
    if !configured {
        return ("\u{2717} not configured".to_string(), state.theme.pending);
    }
    match status {
        None => ("\u{22EF} probing\u{2026}".to_string(), state.theme.running),
        Some(s) if s.connected => ("\u{2713} connected".to_string(), state.theme.ok),
        Some(s) => {
            let lbl = s
                .error
                .as_deref()
                .map(|e| format!("\u{26A0} {}", short(e, 18)))
                .unwrap_or_else(|| "\u{26A0} error".to_string());
            (lbl, state.theme.failed)
        }
    }
}

fn short(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\u{2026}", &s[..max.saturating_sub(1)])
    }
}

fn format_balance(s: Option<&VendorStatus>) -> String {
    match s.and_then(|v| v.balance) {
        Some(b) => format!("${:.2}", b),
        None => "\u{2014}".to_string(),
    }
}

fn format_last_checked(s: Option<&VendorStatus>) -> String {
    let Some(s) = s else {
        return "\u{2014}".to_string();
    };
    let secs = Utc::now()
        .signed_duration_since(s.last_checked)
        .num_seconds()
        .max(0);
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

fn is_configured(state: &AppState, vendor: &str) -> bool {
    match vendor {
        "vast" => state.credentials.vast.api_key.is_some(),
        "kaggle" => {
            state.credentials.kaggle.username.is_some() && state.credentials.kaggle.key.is_some()
        }
        "mlflow" => state.credentials.mlflow.token.is_some(),
        _ => false,
    }
}
