use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::state::{AppState, Screen};

pub fn render(f: &mut Frame, state: &AppState) {
    let area = super::centered_rect(70, 80, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(" Help \u{2014} any key to close ")
        .borders(Borders::ALL)
        .border_style(state.theme.border);

    let screen_help = match &state.screen {
        Screen::Runs => concat!(
            "Runs screen\n",
            "\n",
            "  enter       open run detail\n",
            "  L           launch picker\n",
            "  I           instances\n",
            "  ,           settings\n",
            "  S           stop run (active)\n",
            "  P           pull checkpoint (recent)\n",
            "  R           rerun\n",
            "  /           filter by name\n",
            "  Tab         switch active / recent\n",
            "  g g / G     top / bottom\n",
        ),
        Screen::RunDetail(_, _) => concat!(
            "Run detail\n",
            "\n",
            "  Tab / Shift-Tab   switch tabs\n",
            "  q / Esc           back\n",
            "\n",
            "  Logs tab\n",
            "  Space             toggle autoscroll\n",
            "  k / j             scroll up / down\n",
            "  Home / End        top / bottom\n",
            "\n",
            "  Manifest tab\n",
            "  e                 open in $EDITOR\n",
        ),
        Screen::Launch => concat!(
            "Launch picker\n",
            "\n",
            "  k / j     navigate up / down\n",
            "  enter     launch selected\n",
            "  Esc       back\n",
            "  g g / G   top / bottom\n",
        ),
        Screen::Instances => concat!(
            "Instances\n",
            "\n",
            "  Tab       toggle Local / Remote (vast.ai)\n",
            "  k / j     navigate up / down\n",
            "  D         destroy orphan (local only)\n",
            "  Esc       back\n",
            "  g g / G   top / bottom\n",
        ),
        Screen::Settings => concat!(
            "Settings\n",
            "\n",
            "  k / j     navigate up / down\n",
            "  enter     edit selected\n",
            "  Esc       back / cancel edit\n",
            "  g g / G   top / bottom\n",
        ),
        Screen::Vendors => concat!(
            "Vendors\n",
            "\n",
            "  k / j     navigate up / down\n",
            "  e/enter   edit credentials (masked)\n",
            "  i         import native key file\n",
            "  t         test connection\n",
            "  r         revoke credentials\n",
            "  Esc / q   back\n",
        ),
    };

    let global_help = concat!(
        "Global\n",
        "\n",
        "  q / Esc   quit / back\n",
        "  ?         this help\n",
        "  :         command palette\n",
        "  g g / G   top / bottom (list screens)\n",
        "\n",
    );

    let full_help = format!("{}{}", global_help, screen_help);
    f.render_widget(
        Paragraph::new(full_help)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
