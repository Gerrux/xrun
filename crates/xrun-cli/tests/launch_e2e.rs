use std::path::PathBuf;

use chrono::Utc;
use tempfile::TempDir;
use xrun_core::{store::ListFilter, store::Store, RunStatus};
use xrun_vast::MockVastAdapter;

use xrun_cli::cli::LaunchArgs;
use xrun_cli::commands::launch::run_with_vendor;

fn make_args(manifest_path: PathBuf) -> LaunchArgs {
    LaunchArgs {
        manifest: manifest_path,
        dry_run: false,
        allow_duplicate: false,
        name: None,
        json: false,
        detach: false,
        max_cost: None,
        max_hours: None,
        idle_timeout: None,
        yes: false,
        reuse_instance: None,
        upload_only: false,
        overrides: Vec::new(),
        trace: false,
    }
}

fn write_manifest(path: &std::path::Path) {
    let yaml = "name: e2e-test\nvendor: vast\nvast:\n  image: pytorch/pytorch:latest\n  gpu:\n    type: RTX4090\n    count: 1\nrun:\n  cmd: python train.py\n";
    std::fs::write(path, yaml).unwrap();
}

fn event_line(stage: &str, status: &str) -> Vec<u8> {
    let ts = Utc::now().to_rfc3339();
    format!(r#"{{"ts":"{ts}","stage":"{stage}","status":"{status}"}}"#).into_bytes()
}

fn join_lines(lines: &[Vec<u8>]) -> Vec<u8> {
    lines
        .iter()
        .flat_map(|l| l.iter().copied().chain([b'\n']))
        .collect()
}

/// Full happy-path: all 4 events in a single tail batch, done returned immediately.
#[test]
fn launch_e2e_with_mock_vendor_done() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("runs.db");
    let runs_dir = tmp.path().join("runs");
    let manifest_path = tmp.path().join("test.yaml");

    write_manifest(&manifest_path);

    let events_data = join_lines(&[
        event_line("provision", "ok"),
        event_line("upload", "ok"),
        event_line("train_start", "ok"),
        event_line("done", "ok"),
    ]);

    let mock = MockVastAdapter::new(vec![events_data], vec![]);
    let args = make_args(manifest_path);

    run_with_vendor(&args, &db_path, &runs_dir, Box::new(mock))
        .expect("launch with mock vendor should succeed");

    let store = Store::open(&db_path).unwrap();
    let runs = store.list_runs(&ListFilter::default()).unwrap();
    assert_eq!(runs.len(), 1, "expected exactly one run");

    let run = &runs[0];
    assert_eq!(run.status, RunStatus::Done, "run should be done");

    let events = store.list_events(&run.id).unwrap();
    assert!(
        events.len() >= 4,
        "expected >= 4 events, got {}",
        events.len()
    );
    assert!(
        events.iter().any(|e| e.stage == "done"),
        "done event missing from store"
    );
}

/// Fail-path: a fail event triggers the policy and propagates as Err.
#[test]
fn launch_e2e_mock_vendor_failed_run() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("runs.db");
    let runs_dir = tmp.path().join("runs");
    let manifest_path = tmp.path().join("test.yaml");

    write_manifest(&manifest_path);

    let events_data = join_lines(&[event_line("train_start", "ok"), event_line("train", "fail")]);
    let mock = MockVastAdapter::new(vec![events_data], vec![]);
    let args = make_args(manifest_path);

    let result = run_with_vendor(&args, &db_path, &runs_dir, Box::new(mock));
    assert!(result.is_err(), "failed run should propagate as Err");

    let store = Store::open(&db_path).unwrap();
    let runs = store.list_runs(&ListFilter::default()).unwrap();
    assert_eq!(
        runs[0].status,
        RunStatus::Failed,
        "run status should be failed"
    );
}
