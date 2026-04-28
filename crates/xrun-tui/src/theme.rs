use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub pending: Style,
    pub running: Style,
    pub ok: Style,
    pub failed: Style,
    pub warn: Style,
    pub status_bar: Style,
    pub title: Style,
    pub border: Style,
    pub selected: Style,
    pub normal: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "default",
            pending: Style::default().fg(Color::DarkGray),
            running: Style::default().fg(Color::Yellow),
            ok: Style::default().fg(Color::Green),
            failed: Style::default().fg(Color::Red),
            warn: Style::default().fg(Color::Magenta),
            status_bar: Style::default().fg(Color::Gray).bg(Color::DarkGray),
            title: Style::default().fg(Color::White),
            border: Style::default().fg(Color::DarkGray),
            selected: Style::default().fg(Color::Black).bg(Color::White),
            normal: Style::default(),
        }
    }
}

impl Theme {
    pub fn high_contrast() -> Self {
        Self {
            name: "high_contrast",
            pending: Style::default().fg(Color::Gray),
            running: Style::default().fg(Color::Yellow),
            ok: Style::default().fg(Color::Green),
            failed: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            warn: Style::default().fg(Color::Magenta),
            status_bar: Style::default().fg(Color::White).bg(Color::Black),
            title: Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            border: Style::default().fg(Color::White),
            selected: Style::default().fg(Color::Black).bg(Color::Yellow),
            normal: Style::default().fg(Color::White),
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "high_contrast" => Self::high_contrast(),
            _ => Self::default(),
        }
    }
}
