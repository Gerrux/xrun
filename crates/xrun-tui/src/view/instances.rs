use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use xrun_core::vendor::VendorRemoteInstance;

use crate::state::{AppState, InstancesSection};

use super::anim;
use super::empty::EmptyState;
use super::SPINNER;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    // Adaptive local section height: collapsed when empty
    let local_height: u16 = if state.instances.instances.is_empty() {
        4
    } else {
        (state.instances.instances.len() as u16 + 3).clamp(5, area.height / 2)
    };

    let chunks =
        Layout::vertical([Constraint::Length(local_height), Constraint::Min(0)]).split(area);

    render_local(f, chunks[0], state);
    render_remote(f, chunks[1], state);
}

fn render_local(f: &mut Frame, area: Rect, state: &AppState) {
    let is_focused = state.instances.section == InstancesSection::Local;
    let border_style = if is_focused {
        Style::default().fg(Color::White)
    } else {
        state.theme.border
    };

    let title = format!(" Local ({}) ", state.instances.instances.len());

    if state.instances.instances.is_empty() {
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(Span::styled(
                "no tracked instances \u{2014} run xrun launch to provision",
                state.theme.dim_text,
            ))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let header =
        Row::new(["GPU", "Vendor", "Price/h", "Uptime", "Run", "Status"]).style(state.theme.title);

    let rows: Vec<Row> = state
        .instances
        .instances
        .iter()
        .enumerate()
        .map(|(i, inst)| {
            let selected = is_focused && i == state.instances.selected;
            let row_style = if selected {
                anim::pulse(state.anim_frame, state.theme.selected)
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
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(state.theme.selected);

    let mut tbl_state = TableState::default();
    if is_focused && !state.instances.instances.is_empty() {
        tbl_state.select(Some(
            state
                .instances
                .selected
                .min(state.instances.instances.len() - 1),
        ));
    }
    f.render_stateful_widget(table, area, &mut tbl_state);
}

fn render_remote(f: &mut Frame, area: Rect, state: &AppState) {
    let is_focused = state.instances.section == InstancesSection::Remote;
    let border_style = if is_focused {
        Style::default().fg(Color::White)
    } else {
        state.theme.border
    };

    let title = format!(" Remote (vast.ai) ({}) ", state.instances.remote.len());

    // Probe is ongoing if vast is configured but no status yet
    let is_probing =
        state.credentials.vast.api_key.is_some() && !state.vendor_statuses.contains_key("vast");

    if state.instances.remote.is_empty() {
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        f.render_widget(block, area);

        if is_probing {
            let spin = SPINNER[state.runs.throbber_frame as usize % SPINNER.len()];
            let spin_str = spin.to_string();
            super::empty::render(
                f,
                inner,
                state,
                EmptyState {
                    icon: &spin_str,
                    title: "probing vast.ai\u{2026}",
                    hint: "please wait",
                },
            );
        } else {
            super::empty::render(
                f,
                inner,
                state,
                EmptyState {
                    icon: "\u{29b8}",
                    title: "No instances at vast.ai",
                    hint: "launch a run to provision one",
                },
            );
        }
        return;
    }

    let header = Row::new(["ID", "GPU", "Status", "$/h", "Uptime", "Region", "SSH"])
        .style(state.theme.title);

    let rows: Vec<Row> = state
        .instances
        .remote
        .iter()
        .enumerate()
        .map(|(i, inst)| {
            let selected = is_focused && i == state.instances.selected;
            let row_style = if selected {
                anim::pulse(state.anim_frame, state.theme.selected)
            } else {
                state.theme.normal
            };
            let status_style = match inst.status.as_deref().unwrap_or("") {
                "running" => state.theme.ok,
                "created" | "loading" => state.theme.running,
                "exited" | "stopped" => state.theme.warn,
                _ => state.theme.normal,
            };

            Row::new(vec![
                Cell::from(inst.id.clone()),
                Cell::from(format_gpu(inst)),
                Cell::from(inst.status.clone().unwrap_or_else(|| "\u{2014}".into()))
                    .style(status_style),
                Cell::from(
                    inst.dph_total
                        .map(|d| format!("${:.3}", d))
                        .unwrap_or_else(|| "\u{2014}".into()),
                ),
                Cell::from(format_uptime(inst.uptime_secs)),
                Cell::from(inst.region.clone().unwrap_or_else(|| "\u{2014}".into())),
                Cell::from(inst.ssh.clone().unwrap_or_else(|| "\u{2014}".into())),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Min(12),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(20),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(state.theme.selected);

    let mut tbl_state = TableState::default();
    if is_focused && !state.instances.remote.is_empty() {
        tbl_state.select(Some(
            state
                .instances
                .selected
                .min(state.instances.remote.len() - 1),
        ));
    }
    f.render_stateful_widget(table, area, &mut tbl_state);
}

fn format_gpu(inst: &VendorRemoteInstance) -> String {
    match (&inst.gpu, inst.num_gpus) {
        (Some(g), Some(n)) if n > 1 => format!("{}\u{00d7} {}", n, g),
        (Some(g), _) => g.clone(),
        _ => "\u{2014}".to_string(),
    }
}

fn format_uptime(secs: Option<u64>) -> String {
    match secs {
        None => "\u{2014}".to_string(),
        Some(s) if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        Some(s) if s < 86_400 => format!("{}h{}m", s / 3600, (s % 3600) / 60),
        Some(s) => format!("{}d{}h", s / 86_400, (s % 86_400) / 3600),
    }
}
