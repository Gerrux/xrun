use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use xrun_core::{Run, RunId, RunStatus, Store};
use xrun_tui::{
    screens::runs::{handle_key, RunsAction},
    state::{AppState, Modal, RunSection, RunsState},
    theme::Theme,
    view,
};

fn make_run(name: &str, status: RunStatus) -> Run {
    Run {
        id: RunId::new(),
        name: name.to_string(),
        manifest_hash: "abc".to_string(),
        manifest_path: format!("exp/{}.yaml", name),
        vendor: "vast".to_string(),
        instance_id: None,
        status,
        created_at: Utc::now(),
        started_at: Some(Utc::now()),
        ended_at: None,
        cost_usd: None,
        mlflow_run_id: None,
        notes: None,
        poller_pid: None,
        mlflow_run_url: None,
        wandb_run_id: None,
        wandb_run_url: None,
    }
}

fn state_with_runs(active: usize, recent: usize) -> AppState {
    let mut state = AppState::new(Theme::default());
    state.runs = RunsState {
        active_runs: (0..active)
            .map(|i| make_run(&format!("active-run-{}", i), RunStatus::Running))
            .collect(),
        recent_runs: {
            let statuses = [RunStatus::Done, RunStatus::Failed, RunStatus::Cancelled];
            (0..recent)
                .map(|i| make_run(&format!("recent-run-{}", i), statuses[i % 3].clone()))
                .collect()
        },
        ..Default::default()
    };
    state
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

// ── snapshot ─────────────────────────────────────────────────────────────────

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
fn snapshot_runs_screen_2active_3recent() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut state = state_with_runs(2, 3);
    // freeze throbber frame for deterministic snapshot
    state.runs.throbber_frame = 0;

    terminal.draw(|f| view::render(f, &state)).unwrap();

    let actual = buffer_to_string(terminal.backend().buffer());

    let snap_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let golden_path = snap_dir.join("runs_screen.txt");

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
fn s_on_active_row_creates_stop_modal() {
    let mut state = state_with_runs(2, 3);
    assert_eq!(state.runs.section, RunSection::Active);
    assert_eq!(state.runs.selected, 0);

    let action = handle_key(&mut state, key(KeyCode::Char('S')));

    let first_id = state.runs.active_runs[0].id.clone();
    match action {
        RunsAction::ShowStopConfirm(id, name) => {
            assert_eq!(id, first_id);
            assert_eq!(name, "active-run-0");
        }
        _ => panic!("expected ShowStopConfirm"),
    }
    assert!(
        state.modal.is_none(),
        "modal should not be set by screen handler"
    );
}

#[test]
fn p_on_recent_row_creates_pull_confirm() {
    let mut state = state_with_runs(2, 3);
    // Switch to Recent section
    handle_key(&mut state, key(KeyCode::Tab));
    assert_eq!(state.runs.section, RunSection::Recent);

    let action = handle_key(&mut state, key(KeyCode::Char('P')));
    let first_id = state.runs.recent_runs[0].id.clone();
    match action {
        RunsAction::ShowPullConfirm(id, name) => {
            assert_eq!(id, first_id);
            assert_eq!(name, "recent-run-0");
        }
        _ => panic!("expected ShowPullConfirm"),
    }
}

#[test]
fn enter_on_active_row_returns_open_run_detail() {
    let mut state = state_with_runs(2, 3);
    let first_id = state.runs.active_runs[0].id.clone();

    let action = handle_key(&mut state, key(KeyCode::Enter));
    match action {
        RunsAction::OpenRunDetail(id) => assert_eq!(id, first_id),
        _ => panic!("expected OpenRunDetail"),
    }
}

#[test]
fn slash_opens_filter_input_modal() {
    let mut state = state_with_runs(2, 3);
    assert!(state.modal.is_none());

    let action = handle_key(&mut state, key(KeyCode::Char('/')));
    assert!(matches!(action, RunsAction::Nothing));
    assert!(matches!(
        state.modal,
        Some(xrun_tui::state::Modal::FilterInput { .. })
    ));
}

#[test]
fn filter_input_applies_on_enter() {
    let mut state = state_with_runs(2, 3);
    state.modal = Some(Modal::FilterInput {
        input: "active-run-0".to_string(),
    });

    handle_key(&mut state, key(KeyCode::Enter));

    assert!(state.modal.is_none());
    assert_eq!(state.runs.filter, Some("active-run-0".to_string()));
}

#[test]
fn filter_input_dismissed_on_esc() {
    let mut state = state_with_runs(2, 3);
    state.modal = Some(Modal::FilterInput {
        input: "foo".to_string(),
    });

    handle_key(&mut state, key(KeyCode::Esc));

    assert!(state.modal.is_none());
    assert!(
        state.runs.filter.is_none(),
        "filter should not be applied on Esc"
    );
}

#[test]
fn navigation_wraps_at_bounds() {
    let mut state = state_with_runs(3, 0);

    // Navigate up at top → stays at 0
    handle_key(&mut state, key(KeyCode::Up));
    assert_eq!(state.runs.selected, 0);

    // Navigate down twice
    handle_key(&mut state, key(KeyCode::Down));
    handle_key(&mut state, key(KeyCode::Down));
    assert_eq!(state.runs.selected, 2);

    // Navigate down again at bottom → stays at 2
    handle_key(&mut state, key(KeyCode::Down));
    assert_eq!(state.runs.selected, 2);
}

#[test]
fn tab_toggles_section_and_resets_selection() {
    let mut state = state_with_runs(2, 3);
    state.runs.selected = 1;

    handle_key(&mut state, key(KeyCode::Tab));
    assert_eq!(state.runs.section, RunSection::Recent);
    assert_eq!(state.runs.selected, 0, "selection resets on section switch");

    handle_key(&mut state, key(KeyCode::Tab));
    assert_eq!(state.runs.section, RunSection::Active);
}

// ── store-based stop test ─────────────────────────────────────────────────────

#[test]
fn stop_run_via_confirm_updates_store() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut store = Store::open(&db_path).unwrap();

    let id = store
        .create_run("my-run", "hash", "exp/my-run.yaml", "vast", &[])
        .unwrap();

    store.update_run_status(&id, RunStatus::Running).unwrap();

    // Verify it's running
    let run = store.get_run(&id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Running);

    // Simulate what execute_confirm_action does
    store.update_run_status(&id, RunStatus::Cancelled).unwrap();

    let run_after = store.get_run(&id).unwrap().unwrap();
    assert_eq!(run_after.status, RunStatus::Cancelled);
}
