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
    // v0.2.2 — extended palette
    pub accent: Style,
    pub dim_text: Style,
    pub card_bg: Style,
    pub success_bg: Style,
    pub error_bg: Style,
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
            accent: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            dim_text: Style::default().fg(Color::DarkGray),
            card_bg: Style::default(),
            success_bg: Style::default().fg(Color::Black).bg(Color::Green),
            error_bg: Style::default().fg(Color::White).bg(Color::Red),
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
            accent: Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            dim_text: Style::default().fg(Color::Gray),
            card_bg: Style::default().bg(Color::Black),
            success_bg: Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
            error_bg: Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        }
    }

    /// Nord dark theme — uses RGB colors (requires truecolor terminal).
    /// Falls back to nearest ANSI color on non-truecolor terminals.
    pub fn nord() -> Self {
        Self {
            name: "nord",
            // nord3 = #4C566A (polar night light)
            pending: Style::default().fg(Color::Rgb(76, 86, 106)),
            // nord13 = #EBCB8B (aurora yellow)
            running: Style::default().fg(Color::Rgb(235, 203, 139)),
            // nord14 = #A3BE8C (aurora green)
            ok: Style::default().fg(Color::Rgb(163, 190, 140)),
            // nord11 = #BF616A (aurora red)
            failed: Style::default().fg(Color::Rgb(191, 97, 106)),
            // nord12 = #D08770 (aurora orange)
            warn: Style::default().fg(Color::Rgb(208, 135, 112)),
            // nord4 = #D8DEE9 on nord1 = #3B4252
            status_bar: Style::default()
                .fg(Color::Rgb(216, 222, 233))
                .bg(Color::Rgb(59, 66, 82)),
            // nord6 = #ECEFF4 bold
            title: Style::default()
                .fg(Color::Rgb(236, 239, 244))
                .add_modifier(Modifier::BOLD),
            // nord3 = #4C566A
            border: Style::default().fg(Color::Rgb(76, 86, 106)),
            // nord6 text on nord2 = #434C5E bg
            selected: Style::default()
                .fg(Color::Rgb(236, 239, 244))
                .bg(Color::Rgb(67, 76, 94)),
            // nord4 = #D8DEE9
            normal: Style::default().fg(Color::Rgb(216, 222, 233)),
            // nord8 = #88C0D0 (frost blue) bold — brand accent
            accent: Style::default()
                .fg(Color::Rgb(136, 192, 208))
                .add_modifier(Modifier::BOLD),
            // nord3 = #4C566A (dim labels)
            dim_text: Style::default().fg(Color::Rgb(76, 86, 106)),
            // nord1 = #3B4252
            card_bg: Style::default().bg(Color::Rgb(59, 66, 82)),
            // nord0 on nord14
            success_bg: Style::default()
                .fg(Color::Rgb(46, 52, 64))
                .bg(Color::Rgb(163, 190, 140)),
            // nord6 on nord11
            error_bg: Style::default()
                .fg(Color::Rgb(236, 239, 244))
                .bg(Color::Rgb(191, 97, 106)),
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "high_contrast" => Self::high_contrast(),
            "nord" => Self::nord(),
            _ => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_nord() {
        let t = Theme::from_name("nord");
        assert_eq!(t.name, "nord");
    }

    #[test]
    fn from_name_unknown_falls_back_to_default() {
        let t = Theme::from_name("doesnotexist");
        assert_eq!(t.name, "default");
    }
}
