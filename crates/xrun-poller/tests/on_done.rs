//! `policy.on_done` / `artifacts.pull_on`: what the poller does on a natural
//! `done` — pull artifacts, destroy the instance, mark the run done.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use tempfile::TempDir;
use xrun_core::{
    config::NotifyConfig,
    error::VendorError,
    manifest::{DataSource, DonePolicy, Manifest, RunSpec},
    store::{RunId, RunStatus, Store},
    vendor::{DryRunPlan, InstanceHandle, PollCompletion, VendorAdapter},
};
use xrun_notify::{Channel, Kind, Notification, Notifier, NotifyError};
use xrun_poller::{CancellationToken, Poller, PollerConfig};

#[derive(Clone, Default)]
struct Calls {
    /// "pull:<pattern>" / "destroy", in call order.
    log: Arc<Mutex<Vec<String>>>,
    destroys: Arc<AtomicUsize>,
    run_id: Arc<Mutex<Option<RunId>>>,
}

impl Calls {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

struct MockVendor {
    events: RefCell<VecDeque<Vec<u8>>>,
    calls: Calls,
    fail_pull_pattern: Option<&'static str>,
    destroy_fails: bool,
    metrics: RefCell<VecDeque<Vec<u8>>>,
    /// Heartbeat (as seen in the DB) at the start of every pull.
    heartbeats: Arc<Mutex<Vec<Option<chrono::DateTime<Utc>>>>>,
    db_path: Option<std::path::PathBuf>,
    /// Report `Done` via `poll_completion` (Kaggle-style) instead of a
    /// `done` event.
    completion_done: bool,
}

impl MockVendor {
    fn new(calls: &Calls) -> Self {
        Self {
            events: RefCell::new(vec![done_event()].into()),
            completion_done: false,
            calls: calls.clone(),
            fail_pull_pattern: None,
            destroy_fails: false,
            metrics: RefCell::new(VecDeque::new()),
            heartbeats: Arc::default(),
            db_path: None,
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
        if file.contains("metrics") {
            return Ok(self.metrics.borrow_mut().pop_front().unwrap_or_default());
        }
        if file.contains("stdout") {
            return Ok(Vec::new());
        }
        Ok(self.events.borrow_mut().pop_front().unwrap_or_default())
    }
    fn pull(&self, _: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError> {
        self.calls
            .log
            .lock()
            .unwrap()
            .push(format!("pull:{remote}"));
        let rid = self.calls.run_id.lock().unwrap().clone();
        if let (Some(db), Some(rid)) = (&self.db_path, rid) {
            let hb = Store::open(db)
                .ok()
                .and_then(|s| s.get_run(&rid).ok().flatten())
                .and_then(|r| r.poller_heartbeat_at);
            self.heartbeats.lock().unwrap().push(hb);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if self.fail_pull_pattern == Some(remote) {
            return Err(VendorError::Other("ssh: connection reset".into()));
        }
        std::fs::write(into.join("best.pt"), b"w").unwrap();
        Ok(())
    }
    fn poll_completion(&self, _: &InstanceHandle, _: &Path) -> Option<PollCompletion> {
        self.completion_done.then(|| PollCompletion {
            terminal_status: Some(RunStatus::Done),
            events: Vec::new(),
        })
    }
    fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
        self.calls.log.lock().unwrap().push("destroy".into());
        self.calls.destroys.fetch_add(1, Ordering::SeqCst);
        if self.destroy_fails {
            return Err(VendorError::Other("vendor said no".into()));
        }
        Ok(())
    }
}

fn done_event() -> Vec<u8> {
    let ts = Utc::now().to_rfc3339();
    format!("{{\"ts\":\"{ts}\",\"stage\":\"done\",\"status\":\"ok\"}}\n").into_bytes()
}

fn handle() -> InstanceHandle {
    InstanceHandle {
        id: "inst-1".to_string(),
        vendor: "mock".to_string(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "root".to_string(),
    }
}

fn setup(tmp: &TempDir) -> RunId {
    let mut store = Store::open(&tmp.path().join("runs.db")).unwrap();
    let run_id = store
        .create_run("resnet_v2", "hash", "manifest.yaml", "vast", &[])
        .unwrap();
    store
        .update_run_status(&run_id, RunStatus::Running)
        .unwrap();
    store
        .update_run_started_at(&run_id, Utc::now() - Duration::minutes(5))
        .unwrap();
    run_id
}

fn run(
    tmp: &TempDir,
    run_id: &RunId,
    vendor: MockVendor,
    policy: DonePolicy,
) -> Result<RunStatus, xrun_poller::PollerError> {
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..Default::default()
    })
    .with_done_policy(policy)
    .run(CancellationToken::new())
}

fn policy(stop_instance: bool, patterns: &[&str]) -> DonePolicy {
    DonePolicy {
        stop_instance,
        pull_patterns: patterns.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// Mark the (otherwise absent) instance row so `destroyed_at` can be checked.
fn insert_instance(tmp: &TempDir, run_id: &RunId) {
    Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .insert_instance("inst-1", "mock", Some(run_id), None, None, Utc::now())
        .unwrap();
}

fn destroyed_at_set(tmp: &TempDir) -> bool {
    Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .get_instance("inst-1")
        .unwrap()
        .unwrap()
        .destroyed_at
        .is_some()
}

fn run_status(tmp: &TempDir, run_id: &RunId) -> RunStatus {
    Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .get_run(run_id)
        .unwrap()
        .unwrap()
        .status
}

fn events(tmp: &TempDir, run_id: &RunId) -> Vec<(String, String, Option<String>)> {
    Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .list_events(run_id)
        .unwrap()
        .into_iter()
        .map(|e| (e.stage, e.status, e.msg))
        .collect()
}

#[test]
fn done_pulls_then_destroys() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let status = run(
        &tmp,
        &run_id,
        MockVendor::new(&calls),
        policy(true, &["ckpt/*.pt", "logs/*"]),
    )
    .unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.log(), ["pull:ckpt/*.pt", "pull:logs/*", "destroy"]);
    assert!(tmp
        .path()
        .join("runs")
        .join(run_id.to_string())
        .join("artifacts")
        .join("best.pt")
        .exists());
    let ev = events(&tmp, &run_id);
    let pulls: Vec<_> = ev.iter().filter(|e| e.0 == "artifacts.pull").collect();
    assert_eq!(pulls.len(), 2);
    assert!(pulls.iter().all(|e| e.1 == "ok"));
    assert!(pulls[0].2.as_deref().unwrap().contains("ckpt/*.pt"));
    assert_eq!(run_status(&tmp, &run_id), RunStatus::Done);
}

#[test]
fn done_without_patterns_destroys_without_pull() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let status = run(&tmp, &run_id, MockVendor::new(&calls), policy(true, &[])).unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.log(), ["destroy"]);
    assert!(events(&tmp, &run_id)
        .iter()
        .all(|e| e.0 != "artifacts.pull"));
}

#[test]
fn default_policy_destroys_on_done() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let status = run(
        &tmp,
        &run_id,
        MockVendor::new(&calls),
        DonePolicy::default(),
    )
    .unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.destroys.load(Ordering::SeqCst), 1);
}

#[test]
fn keep_pulls_but_never_destroys() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let status = run(
        &tmp,
        &run_id,
        MockVendor::new(&calls),
        policy(false, &["ckpt/*.pt"]),
    )
    .unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.log(), ["pull:ckpt/*.pt"]);
    assert_eq!(run_status(&tmp, &run_id), RunStatus::Done);
}

#[test]
fn pull_failure_keeps_instance_and_run_is_done() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.fail_pull_pattern = Some("ckpt/*.pt");
    let status = run(
        &tmp,
        &run_id,
        vendor,
        policy(true, &["ckpt/*.pt", "logs/*"]),
    )
    .unwrap();
    assert_eq!(status, RunStatus::Done);
    // Every pattern is still attempted; destroy is never called.
    assert_eq!(calls.log(), ["pull:ckpt/*.pt", "pull:logs/*"]);
    let ev = events(&tmp, &run_id);
    let fail = ev
        .iter()
        .find(|e| e.0 == "artifacts.pull" && e.1 == "fail")
        .expect("failed pull event");
    let msg = fail.2.as_deref().unwrap();
    assert!(
        msg.contains("ckpt/*.pt") && msg.contains("connection reset"),
        "{msg}"
    );
    assert!(ev.iter().any(|e| e.0 == "instance.kept"));
    assert_eq!(run_status(&tmp, &run_id), RunStatus::Done);
}

/// The failed-stage path keeps its own destroy and never auto-pulls.
#[test]
fn failed_run_destroys_once_without_pulling() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let vendor = MockVendor::new(&calls);
    let ts = Utc::now().to_rfc3339();
    vendor.events.replace(
        vec![format!("{{\"ts\":\"{ts}\",\"stage\":\"train\",\"status\":\"fail\"}}\n").into_bytes()]
            .into(),
    );
    let status = run(&tmp, &run_id, vendor, policy(true, &["ckpt/*.pt"])).unwrap();
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(calls.log(), ["destroy"]);
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Notification>>>);

impl Channel for Capture {
    fn name(&self) -> &'static str {
        "capture"
    }
    fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        self.0.lock().unwrap().push(n.clone());
        Ok(())
    }
}

fn run_notified(
    tmp: &TempDir,
    run_id: &RunId,
    vendor: MockVendor,
    policy: DonePolicy,
    cap: &Capture,
) -> RunStatus {
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..Default::default()
    })
    .with_done_policy(policy)
    .with_notifier(Notifier::with_channels(
        NotifyConfig::default(),
        vec![Box::new(cap.clone())],
    ))
    .run(CancellationToken::new())
    .unwrap()
}

/// A billable instance kept alive after a failed pull must not depend on
/// the (optional) watchdog schedule to be noticed: push right away.
#[test]
fn pull_failure_on_billable_instance_pushes_orphan_warning() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .insert_instance(
            "inst-1",
            "vast",
            Some(&run_id),
            Some("RTX_4090"),
            Some(0.5),
            Utc::now() - Duration::hours(2),
        )
        .unwrap();
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.fail_pull_pattern = Some("ckpt/*.pt");
    let cap = Capture::default();
    let status = run_notified(&tmp, &run_id, vendor, policy(true, &["ckpt/*.pt"]), &cap);
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.destroys.load(Ordering::SeqCst), 0);
    let kinds: Vec<Kind> = cap.0.lock().unwrap().iter().map(|n| n.kind).collect();
    assert_eq!(kinds, [Kind::InstanceOrphan, Kind::RunDone]);
    let orphan = cap.0.lock().unwrap()[0].clone();
    assert!(orphan.title.contains("inst-1"), "{}", orphan.title);
    let inst = Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .get_instance("inst-1")
        .unwrap()
        .unwrap();
    assert!(inst.destroyed_at.is_none(), "kept for `xrun pull`");
}

/// Free instances (Kaggle / local / ssh) kept after a failed pull cost
/// nothing: no orphan push, just `run.done`.
#[test]
fn pull_failure_on_free_instance_sends_only_run_done() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.fail_pull_pattern = Some("ckpt/*.pt");
    let cap = Capture::default();
    run_notified(&tmp, &run_id, vendor, policy(true, &["ckpt/*.pt"]), &cap);
    let kinds: Vec<Kind> = cap.0.lock().unwrap().iter().map(|n| n.kind).collect();
    assert_eq!(kinds, [Kind::RunDone]);
}

#[test]
fn vendor_completion_done_pulls_and_destroys_too() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.events = RefCell::new(VecDeque::new());
    vendor.completion_done = true;
    let status = run(&tmp, &run_id, vendor, policy(true, &["out/*"])).unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.log(), ["pull:out/*", "destroy"]);
}

#[test]
fn destroy_failure_still_marks_run_done() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.destroy_fails = true;
    let status = run(&tmp, &run_id, vendor, policy(true, &[])).unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.destroys.load(Ordering::SeqCst), 3, "3 retries");
    assert!(events(&tmp, &run_id)
        .iter()
        .any(|e| e.0 == "instance.cleanup_failed"));
    assert_eq!(run_status(&tmp, &run_id), RunStatus::Done);
}

const GUARD: &str = "/workspace/**/best*";

fn guard_policy(stop_instance: bool) -> DonePolicy {
    DonePolicy {
        stop_instance,
        guard_pattern: Some(GUARD.into()),
        anchor_dir: Some("/workspace".into()),
        ..Default::default()
    }
}

#[test]
fn guard_pulls_best_before_destroying_when_no_patterns() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let status = run(&tmp, &run_id, MockVendor::new(&calls), guard_policy(true)).unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(
        calls.log(),
        [format!("pull:{GUARD}"), "destroy".to_string()]
    );
}

#[test]
fn guard_failure_keeps_instance_and_run_is_done() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.fail_pull_pattern = Some(GUARD);
    let status = run(&tmp, &run_id, vendor, guard_policy(true)).unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.log(), [format!("pull:{GUARD}")]);
    assert!(events(&tmp, &run_id).iter().any(|e| e.0 == "instance.kept"));
}

#[test]
fn guard_not_used_for_keep_or_explicit_patterns() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    run(&tmp, &run_id, MockVendor::new(&calls), guard_policy(false)).unwrap();
    assert!(
        calls.log().is_empty(),
        "keep: nothing is destroyed, no guard"
    );

    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut p = guard_policy(true);
    p.pull_patterns = vec!["/workspace/out/*".into()];
    run(&tmp, &run_id, MockVendor::new(&calls), p).unwrap();
    assert_eq!(calls.log(), ["pull:/workspace/out/*", "destroy"]);
}

#[test]
fn done_push_points_at_local_artifacts_after_pull() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let cap = Capture::default();
    run_notified(
        &tmp,
        &run_id,
        MockVendor::new(&calls),
        policy(true, &["out/*"]),
        &cap,
    );
    let body = cap.0.lock().unwrap()[0].body.clone();
    assert!(
        body.contains("artifacts:") && !body.contains("xrun pull"),
        "{body}"
    );

    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let cap = Capture::default();
    run_notified(
        &tmp,
        &run_id,
        MockVendor::new(&Calls::default()),
        policy(true, &[]),
        &cap,
    );
    assert!(cap.0.lock().unwrap()[0].body.contains("xrun pull"));
}

fn mark_reused(tmp: &TempDir, run_id: &RunId) {
    Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .append_event(
            run_id,
            xrun_core::store::NewEvent {
                ts: Utc::now(),
                stage: "instance.reused".into(),
                status: "ok".into(),
                msg: None,
                payload_json: None,
            },
        )
        .unwrap();
}

#[test]
fn reused_instance_is_kept_unless_on_done_is_explicit() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    mark_reused(&tmp, &run_id);
    insert_instance(&tmp, &run_id);
    let calls = Calls::default();
    let status = run(&tmp, &run_id, MockVendor::new(&calls), guard_policy(true)).unwrap();
    assert_eq!(status, RunStatus::Done);
    assert!(calls.log().is_empty(), "no guard pull, no destroy");
    assert!(!destroyed_at_set(&tmp));

    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    mark_reused(&tmp, &run_id);
    let calls = Calls::default();
    let mut p = guard_policy(true);
    p.explicit_on_done = true;
    run(&tmp, &run_id, MockVendor::new(&calls), p).unwrap();
    assert_eq!(
        calls.log(),
        [format!("pull:{GUARD}"), "destroy".to_string()]
    );
}

#[test]
fn local_ssh_done_marks_destroyed_without_destroy_call() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    insert_instance(&tmp, &run_id);
    let calls = Calls::default();
    // ssh: patterns are still pulled, but never `vendor.destroy`.
    let p = DonePolicy {
        pull_patterns: vec!["out/*".into()],
        kill_remote: false,
        ..Default::default()
    };
    let status = run(&tmp, &run_id, MockVendor::new(&calls), p).unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.log(), ["pull:out/*"]);
    assert!(destroyed_at_set(&tmp));

    // local (policy has no patterns) and no guard: nothing is called at all.
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    insert_instance(&tmp, &run_id);
    let calls = Calls::default();
    let p = DonePolicy {
        kill_remote: false,
        ..guard_policy(true)
    };
    run(&tmp, &run_id, MockVendor::new(&calls), p).unwrap();
    assert!(calls.log().is_empty());
    assert!(destroyed_at_set(&tmp));
}

#[test]
fn pull_stamps_heartbeat_around_each_pull() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    *calls.run_id.lock().unwrap() = Some(run_id.clone());
    let mut vendor = MockVendor::new(&calls);
    vendor.db_path = Some(tmp.path().join("runs.db"));
    let beats = vendor.heartbeats.clone();
    run(&tmp, &run_id, vendor, policy(true, &["a/*", "b/*"])).unwrap();
    let beats = beats.lock().unwrap().clone();
    assert_eq!(beats.len(), 2);
    let (first, second) = (beats[0].expect("stamped"), beats[1].expect("stamped"));
    assert!(second > first, "heartbeat refreshed between pulls");
}

fn metric_line(step: i64, v: f64) -> String {
    format!(
        "{{\"ts\":\"{}\",\"step\":{step},\"key\":\"val_f1\",\"value\":{v}}}\n",
        Utc::now().to_rfc3339()
    )
}

/// Early-stop fires on the second (non-improving) point.
fn early_stop_vendor(calls: &Calls) -> MockVendor {
    let mut v = MockVendor::new(calls);
    v.events = RefCell::new(VecDeque::new());
    v.metrics = RefCell::new(
        vec![format!("{}{}", metric_line(1, 0.8), metric_line(2, 0.7)).into_bytes()].into(),
    );
    v
}

fn run_early_stop(
    tmp: &TempDir,
    run_id: &RunId,
    vendor: MockVendor,
    policy: DonePolicy,
) -> RunStatus {
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..Default::default()
    })
    .with_early_stop(xrun_core::manifest::EarlyStop {
        metric: "val_f1".into(),
        patience: 1,
        mode: xrun_core::manifest::EarlyStopMode::Max,
        min_delta: 0.0,
        pull: true,
        pull_pattern: None,
    })
    .with_done_policy(policy)
    .run(CancellationToken::new())
    .unwrap()
}

#[test]
fn early_stop_pulls_anchored_pattern_plus_artifacts_then_destroys() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut p = guard_policy(true);
    p.pull_patterns = vec!["/workspace/out/*".into()];
    let status = run_early_stop(&tmp, &run_id, early_stop_vendor(&calls), p);
    assert_eq!(status, RunStatus::Done);
    assert_eq!(
        calls.log(),
        [
            "pull:/workspace/**/best*",
            "pull:/workspace/out/*",
            "destroy"
        ]
    );
}

#[test]
fn early_stop_pull_failure_keeps_instance_but_run_is_done() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut vendor = early_stop_vendor(&calls);
    vendor.fail_pull_pattern = Some("/workspace/**/best*");
    let status = run_early_stop(&tmp, &run_id, vendor, guard_policy(true));
    assert_eq!(status, RunStatus::Done);
    assert_eq!(calls.log(), ["pull:/workspace/**/best*"]);
    let ev = events(&tmp, &run_id);
    assert!(ev.iter().any(|e| e.0 == "instance.kept"));
    assert!(ev.iter().any(|e| e.0 == "early_stop"));
}

#[test]
fn early_stop_destroys_even_for_local_and_keep() {
    // Training is still alive: the process must be killed.
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let p = DonePolicy {
        stop_instance: false,
        kill_remote: false,
        ..Default::default()
    };
    run_early_stop(&tmp, &run_id, early_stop_vendor(&calls), p);
    assert_eq!(calls.log(), ["pull:**/best*", "destroy"]);
}

fn fail_event() -> Vec<u8> {
    let ts = Utc::now().to_rfc3339();
    format!("{{\"ts\":\"{ts}\",\"stage\":\"setup\",\"status\":\"fail\"}}\n").into_bytes()
}

fn run_failing(
    tmp: &TempDir,
    run_id: &RunId,
    vendor: MockVendor,
    on_stage_failed: xrun_poller::FailPolicy,
) -> RunStatus {
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        on_stage_failed,
        ..Default::default()
    })
    .run(CancellationToken::new())
    .unwrap()
}

#[test]
fn failed_stage_keep_ends_run_failed_without_destroy() {
    use xrun_poller::FailPolicy;
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.events = RefCell::new(vec![fail_event()].into());
    let status = run_failing(&tmp, &run_id, vendor, FailPolicy::Keep);
    assert_eq!(status, RunStatus::Failed, "never left `running`");
    assert_eq!(run_status(&tmp, &run_id), RunStatus::Failed);
    assert!(calls.log().is_empty(), "keep: no destroy");
}

#[test]
fn failed_stage_stop_instance_and_reprovision_destroy() {
    use xrun_poller::FailPolicy;
    for policy in [FailPolicy::StopInstance, FailPolicy::Reprovision] {
        let tmp = TempDir::new().unwrap();
        let run_id = setup(&tmp);
        let calls = Calls::default();
        let mut vendor = MockVendor::new(&calls);
        vendor.events = RefCell::new(vec![fail_event()].into());
        let status = run_failing(&tmp, &run_id, vendor, policy);
        assert_eq!(status, RunStatus::Failed);
        assert_eq!(calls.log(), ["destroy"]);
    }
}

/// Caps are enforced for any vendor (here: a free, local-style instance whose
/// row carries only an idle cap): destroy, run Failed, `run.idle` push.
#[test]
fn idle_cap_stops_a_silent_local_style_run() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp);
    {
        let mut store = Store::open(&tmp.path().join("runs.db")).unwrap();
        store
            .insert_instance_with_caps(
                "inst-1",
                "local",
                Some(&run_id),
                None,
                None,
                Utc::now() - Duration::hours(1),
                &xrun_core::store::InstanceCaps {
                    idle_timeout_secs: Some(300),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let calls = Calls::default();
    let mut vendor = MockVendor::new(&calls);
    vendor.events = RefCell::new(VecDeque::new());
    let cap = Capture::default();
    let status = run_notified(&tmp, &run_id, vendor, DonePolicy::default(), &cap);
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(calls.log(), ["destroy"]);
    let inst = Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .get_instance("inst-1")
        .unwrap()
        .unwrap();
    assert_eq!(inst.auto_destroyed_reason.as_deref(), Some("idle_timeout"));
    let kinds: Vec<Kind> = cap.0.lock().unwrap().iter().map(|n| n.kind).collect();
    assert_eq!(kinds, [Kind::RunIdle]);
    assert!(events(&tmp, &run_id)
        .iter()
        .any(|e| e.0 == "instance.auto_destroyed"));
}
