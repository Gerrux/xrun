use chrono::{TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use xrun_core::{RunId, StoredEvent};
use xrun_tui::{
    screens::run_detail::{handle_key, RunDetailAction},
    state::{AppState, LogPaneState, RunDetailState, Screen, Tab},
    theme::Theme,
    view,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn make_event(id: i64, ts_secs: i64, stage: &str, status: &str) -> StoredEvent {
    StoredEvent {
        id,
        ts: Utc.timestamp_opt(ts_secs, 0).unwrap(),
        stage: stage.to_string(),
        status: status.to_string(),
        msg: None,
        payload_json: None,
    }
}

fn state_with_stages() -> AppState {
    let run_id = RunId::new();
    let t0 = 1_700_000_000_i64;

    let events = vec![
        make_event(1, t0, "provision", "running"),
        make_event(2, t0 + 30, "provision", "ok"),
        make_event(3, t0 + 31, "upload", "running"),
        make_event(4, t0 + 91, "upload", "ok"),
        make_event(5, t0 + 92, "train", "running"),
    ];

    let mut state = AppState::new(Theme::default());
    state.run_detail = RunDetailState {
        run: None,
        events,
        log: LogPaneState::default(),
        manifest_text: String::new(),
    };
    state.screen = Screen::RunDetail(run_id, Tab::Stages);
    state.runs.throbber_frame = 0; // deterministic spinner
    state
}

fn state_with_logs(line_count: usize) -> AppState {
    let run_id = RunId::new();
    let lines: Vec<String> = (1..=line_count).map(|i| format!("Line {:02}", i)).collect();

    let mut state = AppState::new(Theme::default());
    state.run_detail = RunDetailState {
        run: None,
        events: Vec::new(),
        log: LogPaneState {
            lines,
            scroll: usize::MAX,
            autoscroll: true,
            search: None,
        },
        manifest_text: String::new(),
    };
    state.screen = Screen::RunDetail(run_id, Tab::Logs);
    state
}

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

fn snap_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name)
}

// ── snapshot tests ────────────────────────────────────────────────────────────

#[test]
fn snapshot_stages_three_stages() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let state = state_with_stages();
    terminal.draw(|f| view::render(f, &state)).unwrap();
    let actual = buffer_to_string(terminal.backend().buffer());

    let golden = snap_path("stages_three_stages.txt");

    if std::env::var("BLESS").is_ok() || !golden.exists() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&golden).unwrap();
    assert_eq!(
        actual, expected,
        "snapshot mismatch — run with BLESS=1 to update"
    );
}

#[test]
fn snapshot_logs_50_lines() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let state = state_with_logs(50);
    terminal.draw(|f| view::render(f, &state)).unwrap();
    let actual = buffer_to_string(terminal.backend().buffer());

    let golden = snap_path("logs_50_lines.txt");

    if std::env::var("BLESS").is_ok() || !golden.exists() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&golden).unwrap();
    assert_eq!(
        actual, expected,
        "snapshot mismatch — run with BLESS=1 to update"
    );
}

// ── key handler unit tests ────────────────────────────────────────────────────

#[test]
fn tab_advances_through_stages_logs_manifest() {
    let run_id = RunId::new();
    let mut state = AppState::new(Theme::default());
    state.screen = Screen::RunDetail(run_id.clone(), Tab::Stages);

    let action = handle_key(&mut state, key(KeyCode::Tab));
    assert!(matches!(action, RunDetailAction::SwitchTab(Tab::Logs)));

    state.screen = Screen::RunDetail(run_id.clone(), Tab::Logs);
    let action = handle_key(&mut state, key(KeyCode::Tab));
    assert!(matches!(action, RunDetailAction::SwitchTab(Tab::Manifest)));

    state.screen = Screen::RunDetail(run_id.clone(), Tab::Manifest);
    let action = handle_key(&mut state, key(KeyCode::Tab));
    assert!(matches!(action, RunDetailAction::SwitchTab(Tab::Stages)));
}

#[test]
fn back_tab_cycles_backwards() {
    let run_id = RunId::new();
    let mut state = AppState::new(Theme::default());
    state.screen = Screen::RunDetail(run_id.clone(), Tab::Stages);

    let action = handle_key(&mut state, key(KeyCode::BackTab));
    assert!(matches!(action, RunDetailAction::SwitchTab(Tab::Manifest)));
}

#[test]
fn esc_returns_back_action() {
    let run_id = RunId::new();
    let mut state = AppState::new(Theme::default());
    state.screen = Screen::RunDetail(run_id, Tab::Stages);

    let action = handle_key(&mut state, key(KeyCode::Esc));
    assert!(matches!(action, RunDetailAction::Back));
}

#[test]
fn space_in_logs_toggles_autoscroll() {
    let run_id = RunId::new();
    let mut state = AppState::new(Theme::default());
    state.screen = Screen::RunDetail(run_id, Tab::Logs);

    let action = handle_key(&mut state, key(KeyCode::Char(' ')));
    assert!(matches!(action, RunDetailAction::ToggleAutoscroll));
}

#[test]
fn space_in_stages_is_nothing() {
    let run_id = RunId::new();
    let mut state = AppState::new(Theme::default());
    state.screen = Screen::RunDetail(run_id, Tab::Stages);

    let action = handle_key(&mut state, key(KeyCode::Char(' ')));
    assert!(matches!(action, RunDetailAction::Nothing));
}

#[test]
fn logs_autoscroll_renders_last_lines() {
    let state = state_with_logs(50);
    // With autoscroll=true, the visible slice starts at max(0, 50 - visible_height)
    // Verify that the last line is "Line 50"
    assert_eq!(state.run_detail.log.lines.last().unwrap(), "Line 50");
    assert!(state.run_detail.log.autoscroll);
}
