use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use xrun_tui::{
    screens::{
        launch::{handle_key, LaunchAction},
        runs::{handle_key as runs_handle_key, RunsAction},
    },
    state::{AppState, LaunchManifest, LaunchState, RunsState, Screen},
    theme::Theme,
    view,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn make_manifest(name: &str, content: &str, previously_run: bool) -> LaunchManifest {
    LaunchManifest {
        path: PathBuf::from(format!("exp/{}.yaml", name)),
        name: name.to_string(),
        content: content.to_string(),
        previously_run,
    }
}

fn state_with_manifests() -> AppState {
    let mut state = AppState::new(Theme::default());
    state.launch = LaunchState {
        manifests: vec![
            make_manifest(
                "experiment-a",
                "vendor: vast\ngpu:\n  type: RTX3090\n  count: 1\n",
                true,
            ),
            make_manifest(
                "experiment-b",
                "vendor: vast\ngpu:\n  type: A100\n  count: 2\n",
                false,
            ),
        ],
        selected: 0,
    };
    state.screen = Screen::Launch;
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

// ── snapshot ─────────────────────────────────────────────────────────────────

#[test]
fn snapshot_launch_picker_two_manifests() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let state = state_with_manifests();
    terminal.draw(|f| view::render(f, &state)).unwrap();
    let actual = buffer_to_string(terminal.backend().buffer());

    let snap_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let golden_path = snap_dir.join("launch_picker.txt");

    if std::env::var("BLESS").is_ok() || !golden_path.exists() {
        std::fs::create_dir_all(&snap_dir).unwrap();
        std::fs::write(&golden_path, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&golden_path)
        .unwrap()
        .replace('\r', "");
    assert_eq!(
        actual, expected,
        "snapshot mismatch — run with BLESS=1 to update"
    );
}

// ── key handler unit tests ────────────────────────────────────────────────────

#[test]
fn l_on_runs_screen_returns_open_launch() {
    let mut state = AppState::new(Theme::default());
    state.runs = RunsState::default();

    let action = runs_handle_key(&mut state, key(KeyCode::Char('L')));
    assert!(matches!(action, RunsAction::OpenLaunch));
}

#[test]
fn i_on_runs_screen_returns_open_instances() {
    let mut state = AppState::new(Theme::default());
    let action = runs_handle_key(&mut state, key(KeyCode::Char('I')));
    assert!(matches!(action, RunsAction::OpenInstances));
}

#[test]
fn comma_on_runs_screen_returns_open_settings() {
    let mut state = AppState::new(Theme::default());
    let action = runs_handle_key(&mut state, key(KeyCode::Char(',')));
    assert!(matches!(action, RunsAction::OpenSettings));
}

#[test]
fn launch_esc_returns_back() {
    let mut state = state_with_manifests();
    let action = handle_key(&mut state, key(KeyCode::Esc));
    assert!(matches!(action, LaunchAction::Back));
}

#[test]
fn launch_enter_returns_confirm_with_path() {
    let mut state = state_with_manifests();
    let action = handle_key(&mut state, key(KeyCode::Enter));
    match action {
        LaunchAction::Confirm(path) => {
            assert!(path.contains("experiment-a"));
        }
        _ => panic!("expected Confirm"),
    }
}

#[test]
fn launch_j_navigates_down() {
    let mut state = state_with_manifests();
    assert_eq!(state.launch.selected, 0);

    handle_key(&mut state, key(KeyCode::Char('j')));
    assert_eq!(state.launch.selected, 1);

    // At bottom, stays at 1
    handle_key(&mut state, key(KeyCode::Char('j')));
    assert_eq!(state.launch.selected, 1);
}

#[test]
fn launch_k_navigates_up() {
    let mut state = state_with_manifests();
    state.launch.selected = 1;

    handle_key(&mut state, key(KeyCode::Char('k')));
    assert_eq!(state.launch.selected, 0);

    // At top, stays at 0
    handle_key(&mut state, key(KeyCode::Char('k')));
    assert_eq!(state.launch.selected, 0);
}

#[test]
fn navigation_push_pop_screen_stack() {
    let mut state = AppState::new(Theme::default());
    assert_eq!(state.screen, Screen::Runs);

    // Simulate L key → OpenLaunch action → push screen
    state.push_screen(Screen::Launch);
    assert_eq!(state.screen, Screen::Launch);
    assert_eq!(state.screen_stack.len(), 1);

    // Simulate Esc → Back → pop screen
    state.pop_screen();
    assert_eq!(state.screen, Screen::Runs);
    assert!(state.screen_stack.is_empty());
}
