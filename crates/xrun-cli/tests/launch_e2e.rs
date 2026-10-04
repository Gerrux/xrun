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
        .flat_map(|l| l.iter().copied().chain(*b"\n"))
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

fn done_events() -> Vec<u8> {
    join_lines(&[event_line("train_start", "ok"), event_line("done", "ok")])
}

/// Run a launch against the mock and return the (single) instance row.
fn launch_and_get_instance(
    yaml: &str,
    tweak: impl FnOnce(&mut LaunchArgs),
) -> xrun_core::store::Instance {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("runs.db");
    let manifest_path = tmp.path().join("m.yaml");
    std::fs::write(&manifest_path, yaml).unwrap();
    let mut args = make_args(manifest_path);
    tweak(&mut args);
    let mock = MockVastAdapter::new(vec![done_events()], vec![]);
    run_with_vendor(&args, &db_path, &tmp.path().join("runs"), Box::new(mock)).unwrap();
    let store = Store::open(&db_path).unwrap();
    let mut rows = store.list_instances().unwrap();
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

const LOCAL_YAML: &str = "name: l\nvendor: local\nrun:\n  cmd: python t.py\n";

#[test]
fn caps_for_non_billable_vendor_come_from_manifest_and_cli_only() {
    // Manifest idle only; no `[budget]` default ($10 cost, 8h) leaks in.
    let inst = launch_and_get_instance(
        &format!("{LOCAL_YAML}policy:\n  on_idle_minutes: 7\n"),
        |_| {},
    );
    assert_eq!(inst.idle_timeout_secs, Some(420));
    assert_eq!(inst.max_cost_usd, None);
    assert_eq!(inst.max_lifetime_secs, None);

    // CLI flags win over the manifest and add the other caps.
    let inst = launch_and_get_instance(
        &format!("{LOCAL_YAML}policy:\n  on_idle_minutes: 7\n"),
        |a| {
            a.idle_timeout = Some(2.0);
            a.max_hours = Some(1.5);
            a.max_cost = Some(3.0);
        },
    );
    assert_eq!(inst.idle_timeout_secs, Some(120));
    assert_eq!(inst.max_lifetime_secs, Some(5400));
    assert_eq!(inst.max_cost_usd, Some(3.0));

    // Nothing asked for -> no caps at all.
    let inst = launch_and_get_instance(LOCAL_YAML, |_| {});
    assert_eq!(
        (
            inst.idle_timeout_secs,
            inst.max_cost_usd,
            inst.max_lifetime_secs
        ),
        (None, None, None)
    );
}

/// Kaggle reports no activity to the poller, so an idle cap there would kill
/// every run N minutes after launch: it is dropped (manifest and CLI alike),
/// while the lifetime cap is still persisted. ssh is observed (the poller
/// tails the remote run dir): its idle cap is kept.
#[test]
fn kaggle_gets_lifetime_cap_but_no_idle_cap_ssh_keeps_idle() {
    let ssh = "name: s\nvendor: ssh\nssh:\n  host_alias: box\nrun:\n  cmd: python t.py\n\
               policy:\n  on_idle_minutes: 3\n";
    let kaggle =
        "name: k\nvendor: kaggle\nkaggle:\n  kernel_slug: me/k\nrun:\n  cmd: python t.py\n\
                  policy:\n  on_idle_minutes: 4\n";
    let inst = launch_and_get_instance(kaggle, |a| {
        a.max_hours = Some(2.0);
    });
    assert_eq!(inst.idle_timeout_secs, None);
    assert_eq!(inst.max_lifetime_secs, Some(7200));
    let inst = launch_and_get_instance(kaggle, |a| {
        a.idle_timeout = Some(5.0);
    });
    assert_eq!(inst.idle_timeout_secs, None);

    let inst = launch_and_get_instance(ssh, |a| {
        a.max_hours = Some(2.0);
    });
    assert_eq!(inst.idle_timeout_secs, Some(180));
    assert_eq!(inst.max_lifetime_secs, Some(7200));
    let inst = launch_and_get_instance(ssh, |a| {
        a.idle_timeout = Some(5.0);
    });
    assert_eq!(inst.idle_timeout_secs, Some(300));
}

/// A reused instance carries the previous run's `last_active_at`; the idle
/// anchor must restart at this launch or the cap fires on the first tick.
#[test]
fn reused_instance_restarts_the_idle_anchor() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("runs.db");
    let runs_dir = tmp.path().join("runs");
    let manifest_path = tmp.path().join("m.yaml");
    std::fs::write(
        &manifest_path,
        format!("{LOCAL_YAML}policy:\n  on_idle_minutes: 30\n"),
    )
    .unwrap();

    // First launch: provision + upload only, instance kept alive.
    let mut args = make_args(manifest_path.clone());
    args.upload_only = true;
    run_with_vendor(
        &args,
        &db_path,
        &runs_dir,
        Box::new(MockVastAdapter::new(vec![], vec![])),
    )
    .unwrap();
    let first_run = {
        let mut store = Store::open(&db_path).unwrap();
        let inst = store.list_instances().unwrap().remove(0);
        store
            .update_instance_usage(&inst.id, 0.0, Some(Utc::now() - chrono::Duration::hours(2)))
            .unwrap();
        store
            .list_runs(&ListFilter::default())
            .unwrap()
            .remove(0)
            .id
            .to_string()
    };

    // Second launch reuses it. Tick 1 sees nothing (no events, no stdout),
    // tick 2 sees `done`: a stale anchor would trip the 30 min cap on tick 1.
    let mut args = make_args(manifest_path);
    args.reuse_instance = Some(first_run);
    let mock = MockVastAdapter::new(vec![vec![], vec![], done_events()], vec![]);
    run_with_vendor(&args, &db_path, &runs_dir, Box::new(mock))
        .expect("reused run must not be idle-killed on its first tick");
    let inst = Store::open(&db_path)
        .unwrap()
        .list_instances()
        .unwrap()
        .remove(0);
    assert!(inst.auto_destroyed_reason.is_none());
}

/// vast keeps its own path (the adapter persists caps at provision, with the
/// global defaults); the new generic persist step must not touch it.
#[test]
fn vast_rows_are_not_touched_by_the_generic_caps_step() {
    let mut yaml = String::new();
    write_manifest_string(&mut yaml);
    let inst = launch_and_get_instance(&yaml, |a| {
        a.max_hours = Some(2.0);
    });
    assert_eq!(inst.max_lifetime_secs, None);
}

fn write_manifest_string(out: &mut String) {
    out.push_str("name: e2e-test\nvendor: vast\nvast:\n  image: pytorch/pytorch:latest\n  gpu:\n    type: RTX4090\n    count: 1\nrun:\n  cmd: python train.py\n");
}
