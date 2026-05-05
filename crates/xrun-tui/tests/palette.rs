use ratatui::{backend::TestBackend, Terminal};
use xrun_tui::{
    screens::palette::{compute_completions, parse_command, PaletteAction},
    state::{AppState, Modal, RunsState, Screen},
    theme::Theme,
    view,
};

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

// ── compute_completions ───────────────────────────────────────────────────────

#[test]
fn completions_empty_input_returns_all() {
    let completions = compute_completions("");
    assert!(
        !completions.is_empty(),
        "empty input should return all commands"
    );
    assert!(completions.iter().any(|c| c.starts_with("goto ")));
    assert!(completions.iter().any(|c| c == "quit"));
}

#[test]
fn completions_for_goto_prefix() {
    let completions = compute_completions("goto");
    assert!(completions.iter().any(|c| c.contains("runs")));
    assert!(completions.iter().any(|c| c.contains("instances")));
    assert!(completions.iter().any(|c| c.contains("settings")));
    // Should not include non-goto commands
    assert!(!completions.iter().any(|c| c.starts_with("launch")));
}

#[test]
fn completions_for_theme_prefix() {
    let completions = compute_completions("theme");
    assert!(completions.iter().any(|c| c.contains("default")));
    assert!(completions.iter().any(|c| c.contains("high_contrast")));
}

#[test]
fn completions_no_match_returns_empty() {
    let completions = compute_completions("zzz_no_match");
    assert!(completions.is_empty());
}

#[test]
fn completions_case_insensitive() {
    let lower = compute_completions("goto");
    let upper = compute_completions("GOTO");
    assert_eq!(lower.len(), upper.len());
}

// ── parse_command ─────────────────────────────────────────────────────────────

#[test]
fn parse_quit() {
    let state = AppState::new(Theme::default());
    assert!(matches!(parse_command("quit", &state), PaletteAction::Quit));
    assert!(matches!(
        parse_command("  quit  ", &state),
        PaletteAction::Quit
    ));
}

#[test]
fn parse_goto_settings() {
    let state = AppState::new(Theme::default());
    let action = parse_command("goto settings", &state);
    assert!(matches!(
        action,
        PaletteAction::GotoScreen(Screen::Settings)
    ));
}

#[test]
fn parse_goto_runs() {
    let state = AppState::new(Theme::default());
    let action = parse_command("goto runs", &state);
    assert!(matches!(action, PaletteAction::GotoScreen(Screen::Runs)));
}

#[test]
fn parse_goto_instances() {
    let state = AppState::new(Theme::default());
    let action = parse_command("goto instances", &state);
    assert!(matches!(
        action,
        PaletteAction::GotoScreen(Screen::Instances)
    ));
}

#[test]
fn parse_goto_unknown_returns_nothing() {
    let state = AppState::new(Theme::default());
    let action = parse_command("goto unknown", &state);
    assert!(matches!(action, PaletteAction::Nothing));
}

#[test]
fn parse_launch_path() {
    let state = AppState::new(Theme::default());
    let action = parse_command("launch tests/data/exp/foo.yaml", &state);
    match action {
        PaletteAction::ShowLaunchConfirm(path) => {
            assert_eq!(path, "tests/data/exp/foo.yaml");
        }
        _ => panic!("expected ShowLaunchConfirm"),
    }
}

#[test]
fn parse_theme_default() {
    let state = AppState::new(Theme::default());
    let action = parse_command("theme default", &state);
    match action {
        PaletteAction::ApplyTheme(name) => assert_eq!(name, "default"),
        _ => panic!("expected ApplyTheme"),
    }
}

#[test]
fn parse_theme_high_contrast() {
    let state = AppState::new(Theme::default());
    let action = parse_command("theme high_contrast", &state);
    match action {
        PaletteAction::ApplyTheme(name) => assert_eq!(name, "high_contrast"),
        _ => panic!("expected ApplyTheme"),
    }
}

#[test]
fn parse_unknown_command_returns_nothing() {
    let state = AppState::new(Theme::default());
    let action = parse_command("foobar", &state);
    assert!(matches!(action, PaletteAction::Nothing));
}

// ── stop/pull with run name lookup ────────────────────────────────────────────

#[test]
fn parse_stop_by_name_finds_run() {
    use chrono::Utc;
    use xrun_core::{Run, RunId, RunStatus};

    let run = Run {
        id: RunId::new(),
        name: "my-run".to_string(),
        manifest_hash: "hash".to_string(),
        manifest_path: "exp/my-run.yaml".to_string(),
        vendor: "vast".to_string(),
        instance_id: None,
        status: RunStatus::Running,
        created_at: Utc::now(),
        started_at: None,
        ended_at: None,
        cost_usd: None,
        mlflow_run_id: None,
        notes: None,
        poller_pid: None,
        mlflow_run_url: None,
        wandb_run_id: None,
        wandb_run_url: None,
    };
    let run_id = run.id.clone();

    let mut state = AppState::new(Theme::default());
    state.runs = RunsState {
        active_runs: vec![run],
        ..Default::default()
    };

    let action = parse_command("stop my-run", &state);
    match action {
        PaletteAction::ShowStopConfirm(Some(id), _) => assert_eq!(id, run_id),
        _ => panic!("expected ShowStopConfirm with run id"),
    }
}

// ── modal rendering ───────────────────────────────────────────────────────────

#[test]
fn command_palette_modal_renders_without_panic() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut state = AppState::new(Theme::default());
    state.modal = Some(Modal::CommandPalette {
        input: "goto".to_string(),
        completions: compute_completions("goto"),
        selected_completion: 0,
    });

    terminal.draw(|f| view::render(f, &state)).unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("goto"), "palette should show typed input");
}

#[test]
fn help_modal_renders_without_panic() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut state = AppState::new(Theme::default());
    state.modal = Some(Modal::Help);

    terminal.draw(|f| view::render(f, &state)).unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains('?') || rendered.contains("help") || rendered.contains("Help"));
}

// ── store-level launch test ───────────────────────────────────────────────────

#[test]
fn launch_confirm_action_creates_run_in_store() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut store = xrun_core::Store::open(&db_path).unwrap();

    // Simulate what execute_confirm_action(LaunchRun) does:
    // create a stub run record for the launched manifest
    let manifest_path = "tests/data/exp/foo.yaml";
    let name = std::path::Path::new(manifest_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("launched");

    let id = store
        .create_run(name, "stub", manifest_path, "vast", &[])
        .unwrap();

    let runs = store.list_runs(&xrun_core::ListFilter::default()).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, id);
    assert_eq!(runs[0].name, "foo");
    assert_eq!(runs[0].manifest_path, manifest_path);
}
