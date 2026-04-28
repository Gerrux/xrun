use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::state::{AppState, Modal};
use crate::view::SPINNER;

const LOGO: &[&str] = &[
    "    __  ___ ____  __  ____   __",
    "    \\ \\/ / __ \\/ / / / / |/ /",
    "     \\  / /_/ / /_/ /  /    /",
    "     /\\ \\\\_, /\\____/  /_/|_/",
    "    /_/\\_\\/_/                ",
];

pub(super) fn render(f: &mut Frame, state: &AppState) {
    let area = super::centered_rect(60, 50, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(state.theme.accent)
        .title(Span::styled(" xrun ", state.theme.accent));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Determine animation frame from elapsed time since splash started.
    let frame: u64 = if let Some(Modal::Splash { started_at, .. }) = &state.modal {
        (started_at.elapsed().as_millis() / 100) as u64
    } else {
        // Splash already dismissed or wrong modal — show full idle state.
        30
    };

    render_animated(f, inner, state, frame);
}

fn render_animated(f: &mut Frame, area: Rect, state: &AppState, frame: u64) {
    // Phase 0 (frames 0–9): logo builds line-by-line, 1 line per 2 frames.
    // Phase 1 (frames 10–15): full logo, accent highlight.
    // Phase 2 (frame 16+): full logo + throbber + "press any key".
    let visible_lines = (frame / 2).min(LOGO.len() as u64) as usize;
    let show_idle = frame >= 16;
    let show_skip_hint = frame >= 4; // show hint early so users can skip

    let logo_style = if frame >= 10 {
        state.theme.accent
    } else {
        // Logo lines that have appeared get accent; rest get nothing (empty line shown)
        state.theme.accent
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("")); // top padding

    for (i, row) in LOGO.iter().enumerate() {
        if i < visible_lines {
            lines.push(Line::from(Span::styled(*row, logo_style)));
        } else {
            lines.push(Line::from("")); // placeholder keeps layout stable
        }
    }

    lines.push(Line::from(""));

    if show_idle {
        let spin = SPINNER[state.runs.throbber_frame as usize % SPINNER.len()];
        lines.push(Line::from(Span::styled(
            format!("{}  loading vendors\u{2026}", spin),
            state.theme.running,
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "press any key to continue",
            state.theme.dim_text,
        )));
    } else if show_skip_hint {
        lines.push(Line::from(Span::styled(
            "press any key to skip",
            state.theme.dim_text,
        )));
    }

    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}
