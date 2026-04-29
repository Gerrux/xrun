use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::state::{AppState, EditField};

pub(super) fn render(
    f: &mut Frame,
    state: &AppState,
    vendor: &str,
    fields: &[EditField],
    focus: usize,
    flash: Option<&str>,
) {
    let area = super::centered_rect(60, 50, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Configure '{}' ", vendor))
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let prefix = if i == focus { "> " } else { "  " };
        let value_repr = render_value(field, i == focus);
        let label_style = if i == focus {
            state.theme.title
        } else {
            state.theme.normal
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{prefix}{:>10}: ", field.label), label_style),
            Span::raw(value_repr),
        ]));
    }
    lines.push(Line::from(""));
    if let Some(msg) = flash {
        lines.push(Line::from(Span::styled(msg.to_string(), state.theme.warn)));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "Tab:next  Shift+Tab:prev  Enter:save  Esc:cancel".to_string(),
        state.theme.status_bar,
    )));

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(para, inner);
}

fn render_value(field: &EditField, focused: bool) -> String {
    let display = if field.secret {
        "\u{2022}".repeat(field.value.chars().count())
    } else {
        field.value.clone()
    };
    if focused {
        format!("{}_", display)
    } else if display.is_empty() {
        "(empty)".to_string()
    } else {
        display
    }
}
