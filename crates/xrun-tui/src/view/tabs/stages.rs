use chrono::{DateTime, Utc};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};
use xrun_core::StoredEvent;

use crate::state::AppState;

use crate::view::SPINNER;

struct StageData {
    name: String,
    status: String,
    first_ts: DateTime<Utc>,
    last_ts: DateTime<Utc>,
}

fn aggregate_stages(events: &[StoredEvent]) -> Vec<StageData> {
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, StageData> = std::collections::HashMap::new();

    for event in events {
        if let Some(data) = map.get_mut(&event.stage) {
            data.last_ts = event.ts;
            data.status = event.status.clone();
        } else {
            order.push(event.stage.clone());
            map.insert(
                event.stage.clone(),
                StageData {
                    name: event.stage.clone(),
                    status: event.status.clone(),
                    first_ts: event.ts,
                    last_ts: event.ts,
                },
            );
        }
    }

    order.into_iter().map(|n| map.remove(&n).unwrap()).collect()
}

fn format_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let stages = aggregate_stages(&state.run_detail.events);
    let spin_ch = SPINNER[state.runs.throbber_frame as usize % SPINNER.len()];

    let items: Vec<ListItem> = if stages.is_empty() {
        vec![ListItem::new(Span::styled(
            "  (no events yet)",
            state.theme.normal,
        ))]
    } else {
        stages
            .iter()
            .map(|stage| {
                let (icon, style) = match stage.status.as_str() {
                    "ok" | "done" | "success" => ("✓", state.theme.ok),
                    "failed" | "error" => ("✗", state.theme.failed),
                    "running" => (spin_ch, state.theme.running),
                    _ => ("·", state.theme.normal),
                };

                let is_terminal = matches!(
                    stage.status.as_str(),
                    "ok" | "done" | "success" | "failed" | "error"
                );
                let duration_str = if is_terminal {
                    let secs = stage
                        .last_ts
                        .signed_duration_since(stage.first_ts)
                        .num_seconds()
                        .max(0);
                    format!("  {}", format_duration(secs))
                } else {
                    String::new()
                };

                let line = Line::from(vec![
                    Span::styled(format!("  {} ", icon), style),
                    Span::styled(stage.name.clone(), style),
                    Span::styled(duration_str, state.theme.normal),
                ]);
                ListItem::new(line)
            })
            .collect()
    };

    let list = List::new(items).block(
        Block::default()
            .title(" Stages ")
            .borders(Borders::ALL)
            .border_style(state.theme.border),
    );

    f.render_widget(list, area);
}
