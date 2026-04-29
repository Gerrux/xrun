use ratatui::{backend::TestBackend, Terminal};
use xrun_tui::{state::AppState, theme::Theme, view};

fn buffer_to_string(buffer: &ratatui::buffer::Buffer) -> String {
    let area = buffer.area();
    let mut result = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            result.push_str(buffer[(x, y)].symbol());
        }
        result.push('\n');
    }
    result
}

#[test]
fn smoke_empty_state() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let state = AppState::new(Theme::default());
    terminal.draw(|f| view::render(f, &state)).unwrap();

    let actual = buffer_to_string(terminal.backend().buffer());

    let snap_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let golden_path = snap_dir.join("empty.txt");

    if std::env::var("BLESS").is_ok() || !golden_path.exists() {
        std::fs::create_dir_all(&snap_dir).unwrap();
        std::fs::write(&golden_path, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&golden_path).unwrap();
    assert_eq!(
        actual, expected,
        "snapshot mismatch — run with BLESS=1 to update"
    );
}
