use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;

use chrono::Utc;
use tempfile::TempDir;
use xrun_core::{
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{RunId, RunStatus, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};
use xrun_poller::{
    lock::{PollerLock, PollerLockError},
    parser::{parse_events, parse_metrics},
    CancellationToken, Poller, PollerConfig, PollerError,
};

// ---------------------------------------------------------------------------
// Mock vendor: returns scripted tail data for events and metrics files.
// ---------------------------------------------------------------------------

struct MockVendor {
    events_queue: RefCell<VecDeque<Vec<u8>>>,
    metrics_queue: RefCell<VecDeque<Vec<u8>>>,
}

impl MockVendor {
    fn new(events: Vec<Vec<u8>>, metrics: Vec<Vec<u8>>) -> Self {
        Self {
            events_queue: RefCell::new(events.into()),
            metrics_queue: RefCell::new(metrics.into()),
        }
    }
}

impl VendorAdapter for MockVendor {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn validate(&self, _: &Manifest) -> Result<(), VendorError> {
        Ok(())
    }

    fn dry_run_plan(&self, _: &Manifest) -> Result<DryRunPlan, VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn provision(&self, _: &Manifest) -> Result<InstanceHandle, VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn upload(&self, _: &InstanceHandle, _: &[DataSource]) -> Result<(), VendorError> {
        Ok(())
    }

    fn execute(&self, _: &InstanceHandle, _: &RunSpec) -> Result<(), VendorError> {
        Ok(())
    }

    fn tail(&self, _h: &InstanceHandle, file: &str, _offset: u64) -> Result<Vec<u8>, VendorError> {
        let queue = if file.contains("metrics") {
            &self.metrics_queue
        } else if file.contains("stdout") {
            return Ok(vec![]);
        } else {
            &self.events_queue
        };
        Ok(queue.borrow_mut().pop_front().unwrap_or_default())
    }

    fn pull(&self, _: &InstanceHandle, _: &str, _: &Path) -> Result<(), VendorError> {
        Ok(())
    }

    fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup_store(tmp: &TempDir) -> (Store, RunId) {
    let db = tmp.path().join("runs.db");
    let mut store = Store::open(&db).unwrap();
    let run_id = store
        .create_run("test", "hash", "manifest.yaml", "vast", &[])
        .unwrap();
    (store, run_id)
}

fn make_handle() -> InstanceHandle {
    InstanceHandle {
        id: "999".to_string(),
        vendor: "mock".to_string(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "root".to_string(),
    }
}

fn event_line(stage: &str, status: &str) -> Vec<u8> {
    let ts = Utc::now().to_rfc3339();
    format!(r#"{{"ts":"{ts}","stage":"{stage}","status":"{status}"}}"#).into_bytes()
}

fn metric_line(key: &str, step: i64, value: f64) -> Vec<u8> {
    let ts = Utc::now().to_rfc3339();
    format!(r#"{{"ts":"{ts}","step":{step},"key":"{key}","value":{value}}}"#).into_bytes()
}

fn join_lines(lines: &[Vec<u8>]) -> Vec<u8> {
    lines
        .iter()
        .flat_map(|l| l.iter().copied().chain([b'\n']))
        .collect()
}

fn fast_config() -> PollerConfig {
    PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_poller_run_three_events_to_done() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup_store(&tmp);
    let runs_dir = tmp.path().join("runs");

    let events_data = join_lines(&[
        event_line("train_start", "ok"),
        event_line("epoch", "ok"),
        event_line("done", "ok"),
    ]);

    let mock = MockVendor::new(vec![events_data], vec![]);
    let cancel = CancellationToken::new();

    let status = Poller::new(
        run_id.clone(),
        store,
        Box::new(mock),
        make_handle(),
        runs_dir,
    )
    .with_config(fast_config())
    .run(cancel)
    .unwrap();

    assert_eq!(status, RunStatus::Done);

    let store2 = Store::open(&tmp.path().join("runs.db")).unwrap();
    let events = store2.list_events(&run_id).unwrap();
    assert_eq!(events.len(), 3, "expected 3 events in store");
    assert_eq!(events[0].stage, "train_start");
    assert_eq!(events[1].stage, "epoch");
    assert_eq!(events[2].stage, "done");
}

#[test]
fn test_poller_run_metrics_written() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup_store(&tmp);
    let runs_dir = tmp.path().join("runs");

    let metrics_data = join_lines(&[
        metric_line("val_f1", 1, 0.1),
        metric_line("val_f1", 2, 0.2),
        metric_line("val_f1", 3, 0.3),
        metric_line("val_f1", 4, 0.4),
        metric_line("val_f1", 5, 0.5),
    ]);
    let done_event = join_lines(&[event_line("done", "ok")]);

    // First events call returns empty; done event arrives on the second call.
    let mock = MockVendor::new(vec![vec![], done_event], vec![metrics_data]);
    let cancel = CancellationToken::new();

    let status = Poller::new(
        run_id.clone(),
        store,
        Box::new(mock),
        make_handle(),
        runs_dir,
    )
    .with_config(fast_config())
    .run(cancel)
    .unwrap();

    assert_eq!(status, RunStatus::Done);

    let store2 = Store::open(&tmp.path().join("runs.db")).unwrap();
    let metrics = store2.list_metrics(&run_id, None).unwrap();
    assert_eq!(metrics.len(), 5, "expected 5 metric rows in store");
    assert!(metrics.iter().all(|m| m.key == "val_f1"));
}

#[test]
fn test_poller_drains_metrics_in_same_tick_as_done() {
    // Regression: a fast run (e.g. xrun-local 200ms job) writes its events
    // and metrics in the same window. Before the fix the poller saw `done:ok`
    // in the events tail and returned immediately, so the metric tail in the
    // same iteration never ran. After the fix the metrics are drained first,
    // then the terminal status returns at the end of the tick.
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup_store(&tmp);
    let runs_dir = tmp.path().join("runs");

    // Both events (including done:ok) AND metrics arrive on the very first
    // tail call.
    let events_data = join_lines(&[
        event_line("train_start", "ok"),
        event_line("done", "ok"),
    ]);
    let metrics_data = join_lines(&[
        metric_line("loss", 0, 1.0),
        metric_line("loss", 1, 0.5),
        metric_line("val_f1", 0, 0.7),
    ]);

    let mock = MockVendor::new(vec![events_data], vec![metrics_data]);
    let cancel = CancellationToken::new();

    let status = Poller::new(
        run_id.clone(),
        store,
        Box::new(mock),
        make_handle(),
        runs_dir,
    )
    .with_config(fast_config())
    .run(cancel)
    .unwrap();

    assert_eq!(status, RunStatus::Done);

    let store2 = Store::open(&tmp.path().join("runs.db")).unwrap();
    let metrics = store2.list_metrics(&run_id, None).unwrap();
    assert_eq!(
        metrics.len(),
        3,
        "all metrics from the same-tick window must land in the store"
    );
    let events = store2.list_events(&run_id).unwrap();
    assert_eq!(events.len(), 2);
}

#[test]
fn test_poller_lock_prevents_duplicate() {
    let tmp = TempDir::new().unwrap();
    // Generate a fresh ID each run to avoid cross-test pollution in the global registry.
    let run_id_obj = RunId::new();
    let run_id = run_id_obj.to_string();

    let pid_file1 = tmp.path().join("run_a").join("poller.pid");
    let pid_file2 = tmp.path().join("run_b").join("poller.pid");

    let _lock1 = PollerLock::try_acquire(&run_id, pid_file1).expect("first acquire should succeed");

    let result = PollerLock::try_acquire(&run_id, pid_file2);
    assert!(
        matches!(result, Err(PollerLockError::AlreadyPolling)),
        "second acquire for same run_id should return AlreadyPolling"
    );
    // _lock1 is dropped here, releasing the registry entry.
}

#[test]
fn test_poller_run_returns_already_polling_error() {
    let tmp = TempDir::new().unwrap();
    let (_store, run_id) = setup_store(&tmp);
    let runs_dir = tmp.path().join("runs");

    // Manually hold the lock for this run_id so Poller::run fails.
    let pid_file = runs_dir.join(run_id.to_string()).join("poller.pid");
    std::fs::create_dir_all(pid_file.parent().unwrap()).unwrap();
    let _lock = PollerLock::try_acquire(&run_id.to_string(), pid_file).unwrap();

    let store2 = Store::open(&tmp.path().join("runs.db")).unwrap();
    let mock = MockVendor::new(vec![], vec![]);
    let cancel = CancellationToken::new();

    let result = Poller::new(run_id, store2, Box::new(mock), make_handle(), runs_dir)
        .with_config(fast_config())
        .run(cancel);

    assert!(
        matches!(result, Err(PollerError::AlreadyPolling)),
        "Poller::run should return AlreadyPolling when lock is held, got: {:?}",
        result
    );
}

#[test]
fn test_parser_skips_corrupt_lines() {
    let ts = Utc::now().to_rfc3339();
    let valid1 = format!(r#"{{"ts":"{ts}","stage":"train_start","status":"ok"}}"#);
    let invalid = "NOT_VALID_JSON }{{{";
    let valid2 = format!(r#"{{"ts":"{ts}","stage":"done","status":"ok"}}"#);
    let data = format!("{valid1}\n{invalid}\n{valid2}\n").into_bytes();

    let events = parse_events(&data);
    assert_eq!(
        events.len(),
        2,
        "should parse 2 valid events and skip the corrupt line"
    );
    assert_eq!(events[0].stage, "train_start");
    assert_eq!(events[1].stage, "done");
}

#[test]
fn test_parser_metrics_skips_corrupt_lines() {
    let ts = Utc::now().to_rfc3339();
    let valid1 = format!(r#"{{"ts":"{ts}","step":1,"key":"loss","value":0.5}}"#);
    let invalid = "{bad json]";
    let valid2 = format!(r#"{{"ts":"{ts}","step":2,"key":"loss","value":0.3}}"#);
    let data = format!("{valid1}\n{invalid}\n{valid2}\n").into_bytes();

    let metrics = parse_metrics(&data);
    assert_eq!(
        metrics.len(),
        2,
        "should parse 2 valid metrics and skip the corrupt line"
    );
    assert_eq!(metrics[0].step, 1);
    assert_eq!(metrics[1].step, 2);
}
