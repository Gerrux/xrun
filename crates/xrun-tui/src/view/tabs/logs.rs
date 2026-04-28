use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let log = &state.run_detail.log;
    let inner_height = area.height.saturating_sub(2) as usize; // subtract block borders

    let total = log.lines.len();
    let start = if log.autoscroll || log.scroll >= total {
        total.saturating_sub(inner_height)
    } else {
        log.scroll.min(total.saturating_sub(inner_height))
    };

    let visible: Vec<Line> = log.lines[start..]
        .iter()
        .take(inner_height)
        .map(|line| {
            if let Some(ref query) = log.search {
                if !query.is_empty() {
                    let ql = query.to_lowercase();
                    if let Some(pos) = line.to_lowercase().find(&ql) {
                        return Line::from(vec![
                            Span::raw(line[..pos].to_string()),
                            Span::styled(
                                line[pos..pos + query.len()].to_string(),
                                Style::default().fg(Color::Black).bg(Color::Yellow),
                            ),
                            Span::raw(line[pos + query.len()..].to_string()),
                        ]);
                    }
                }
            }
            Line::from(line.as_str())
        })
        .collect();

    let autoscroll_indicator = if log.autoscroll {
        " [auto] "
    } else {
        " [paused] "
    };
    let title = format!(
        " Logs ({}/{}){}",
        start + visible.len().min(1).max(visible.len()),
        total,
        autoscroll_indicator,
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    f.render_widget(Paragraph::new(visible).block(block), area);
}
