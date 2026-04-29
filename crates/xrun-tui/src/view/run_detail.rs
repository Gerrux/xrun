use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use crate::state::{AppState, Screen, Tab};

use super::tabs;

pub(super) fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let Screen::RunDetail(_, current_tab) = &state.screen else {
        return;
    };

    let run_name = state
        .run_detail
        .run
        .as_ref()
        .map(|r| r.name.as_str())
        .unwrap_or("(unknown)");

    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    let selected = match current_tab {
        Tab::Stages => 0,
        Tab::Logs => 1,
        Tab::Manifest => 2,
    };
    let tabs_widget = Tabs::new(vec!["Stages", "Logs", "Manifest"])
        .select(selected)
        .block(
            Block::default()
                .title(format!(" {} ", run_name))
                .borders(Borders::ALL)
                .border_style(state.theme.border),
        )
        .highlight_style(state.theme.selected)
        .divider("|");
    f.render_widget(tabs_widget, chunks[0]);

    match current_tab {
        Tab::Stages => tabs::stages::render(f, chunks[1], state),
        Tab::Logs => tabs::logs::render(f, chunks[1], state),
        Tab::Manifest => tabs::manifest::render(f, chunks[1], state),
    }

    let hints = match current_tab {
        Tab::Stages => "tab/shift-tab:switch  q/Esc:back",
        Tab::Logs => "tab/shift-tab:switch  Space:autoscroll  Home/End:top/bot  q/Esc:back",
        Tab::Manifest => "tab/shift-tab:switch  e:edit in $EDITOR  q/Esc:back",
    };
    f.render_widget(
        Paragraph::new(hints).style(state.theme.status_bar),
        chunks[2],
    );
}
