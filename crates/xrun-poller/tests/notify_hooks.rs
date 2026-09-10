//! Poller → notifier hooks. A capturing channel stands in for ntfy et al.
//! so we can assert *which* notifications fire for a given event stream.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use tempfile::TempDir;
use xrun_core::{
    config::NotifyConfig,
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{InstanceCaps, RunId, RunStatus, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};
use xrun_notify::{Channel, Kind, Notification, Notifier, NotifyError};
use xrun_poller::{CancellationToken, Poller, PollerConfig};

// ---------------------------------------------------------------------------
// Capturing channel
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Notification>>>);

impl Capture {
    fn kinds(&self) -> Vec<Kind> {
        self.0.lock().unwrap().iter().map(|n| n.kind).collect()
    }
    fn find(&self, kind: Kind) -> Option<Notification> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|n| n.kind == kind)
            .cloned()
    }
}

impl Channel for Capture {
    fn name(&self) -> &'static str {
        "capture"
    }
    fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        self.0.lock().unwrap().push(n.clone());
        Ok(())
    }
}

fn notifier(cap: &Capture) -> Notifier {
    Notifier::with_channels(NotifyConfig::default(), vec![Box::new(cap.clone())])
}

// ---------------------------------------------------------------------------
// Mock vendor (scripted tails)
// ---------------------------------------------------------------------------

struct MockVendor {
    events: RefCell<VecDeque<Vec<u8>>>,
    metrics: RefCell<VecDeque<Vec<u8>>>,
    destroy_fails: bool,
}

impl MockVendor {
    fn new(events: Vec<Vec<u8>>, metrics: Vec<Vec<u8>>) -> Self {
        Self {
            events: RefCell::new(events.into()),
            metrics: RefCell::new(metrics.into()),
            destroy_fails: false,
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
        let q = if file.contains("metrics") {
            &self.metrics
        } else if file.contains("stdout") {
            return Ok(Vec::new());
        } else {
            &self.events
        };
        Ok(q.borrow_mut().pop_front().unwrap_or_default())
    }
    fn pull(&self, _: &InstanceHandle, _: &str, _: &Path) -> Result<(), VendorError> {
        Ok(())
    }
    fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
        if self.destroy_fails {
            return Err(VendorError::Other("vendor said no".into()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn handle() -> InstanceHandle {
    InstanceHandle {
        id: "inst-1".to_string(),
        vendor: "mock".to_string(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "root".to_string(),
    }
}

fn setup(tmp: &TempDir) -> (Store, RunId) {
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
    (store, run_id)
}

fn ev(stage: &str, status: &str, msg: Option<&str>) -> Vec<u8> {
    let ts = Utc::now().to_rfc3339();
    let msg = msg.map(|m| format!(r#","msg":"{m}""#)).unwrap_or_default();
    format!(r#"{{"ts":"{ts}","stage":"{stage}","status":"{status}"{msg}}}"#).into_bytes()
}

fn metric(key: &str, step: i64, value: &str) -> Vec<u8> {
    let ts = Utc::now().to_rfc3339();
    format!(r#"{{"ts":"{ts}","step":{step},"key":"{key}","value":{value}}}"#).into_bytes()
}

fn lines(v: &[Vec<u8>]) -> Vec<u8> {
    v.iter()
        .flat_map(|l| l.iter().copied().chain(*b"\n"))
        .collect()
}

fn fast() -> PollerConfig {
    PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..Default::default()
    }
}

fn run_poller(tmp: &TempDir, store: Store, run_id: RunId, vendor: MockVendor, cap: &Capture) {
    let _ = Poller::new(
        run_id,
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(fast())
    .with_notifier(notifier(cap))
    .run(CancellationToken::new());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn done_event_sends_run_done_with_pull_hint() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    let vendor = MockVendor::new(vec![lines(&[ev("done", "ok", None)])], vec![]);
    run_poller(&tmp, store, run_id.clone(), vendor, &cap);

    assert_eq!(cap.kinds(), vec![Kind::RunDone]);
    let n = cap.find(Kind::RunDone).unwrap();
    assert!(n.title.contains("resnet_v2"), "{}", n.title);
    assert!(
        n.body.contains(&format!("xrun pull {run_id}")),
        "{}",
        n.body
    );
    assert_eq!(n.run_id.as_deref(), Some(run_id.to_string().as_str()));

    // Journaled.
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    let log = store.list_notify_log(None, 10).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, "run.done");
    assert_eq!(log[0].channel, "capture");
    assert!(log[0].ok);
    // Heartbeat was stamped.
    assert!(store
        .get_run(&run_id)
        .unwrap()
        .unwrap()
        .poller_heartbeat_at
        .is_some());
}

#[test]
fn fail_event_sends_run_failed_with_stage_and_msg() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    let vendor = MockVendor::new(
        vec![lines(&[ev("train", "fail", Some("CUDA out of memory"))])],
        vec![],
    );
    run_poller(&tmp, store, run_id, vendor, &cap);

    assert_eq!(cap.kinds(), vec![Kind::RunFailed]);
    let n = cap.find(Kind::RunFailed).unwrap();
    assert!(n.body.contains("train: CUDA out of memory"), "{}", n.body);
}

#[test]
fn nan_metric_sends_one_anomaly_then_done() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    let vendor = MockVendor::new(
        vec![Vec::new(), Vec::new(), lines(&[ev("done", "ok", None)])],
        vec![
            lines(&[
                metric("train_loss", 1, "0.9"),
                metric("train_loss", 2, "NaN"),
            ]),
            lines(&[metric("train_loss", 3, "NaN")]),
        ],
    );
    run_poller(&tmp, store, run_id, vendor, &cap);

    assert_eq!(cap.kinds(), vec![Kind::MetricAnomaly, Kind::RunDone]);
    let n = cap.find(Kind::MetricAnomaly).unwrap();
    assert!(n.title.contains("train_loss"), "{}", n.title);
    assert!(n.body.contains("--key train_loss"), "{}", n.body);
}

#[test]
fn cost_thresholds_fire_once_each_in_order() {
    let tmp = TempDir::new().unwrap();
    let (mut store, run_id) = setup(&tmp);
    // $1/h instance created 54 min ago with a $1 cap → 90% spent at tick 1.
    store
        .insert_instance_with_caps(
            "inst-1",
            "mock",
            Some(&run_id),
            Some("RTX_4090"),
            Some(1.0),
            Utc::now() - Duration::minutes(54),
            &InstanceCaps {
                max_lifetime_secs: None,
                max_cost_usd: Some(1.0),
                idle_timeout_secs: None,
            },
        )
        .unwrap();
    store.update_run_instance_id(&run_id, "inst-1").unwrap();
    let cap = Capture::default();
    // Three quiet ticks, then done — thresholds must not repeat.
    let vendor = MockVendor::new(
        vec![
            Vec::new(),
            Vec::new(),
            Vec::new(),
            lines(&[ev("done", "ok", None)]),
        ],
        vec![],
    );
    run_poller(&tmp, store, run_id, vendor, &cap);

    let kinds = cap.kinds();
    let warns: Vec<Notification> = cap
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|n| n.kind == Kind::BudgetWarn)
        .cloned()
        .collect();
    assert_eq!(warns.len(), 2, "50% and 80% once each; got {kinds:?}");
    assert!(warns[0].title.contains("50%"), "{}", warns[0].title);
    assert!(warns[1].title.contains("80%"), "{}", warns[1].title);
    assert_ne!(warns[0].dedupe_key, warns[1].dedupe_key);
    assert_eq!(*kinds.last().unwrap(), Kind::RunDone);
}

#[test]
fn cost_cap_breach_sends_auto_destroyed() {
    let tmp = TempDir::new().unwrap();
    let (mut store, run_id) = setup(&tmp);
    // Already over the cap at first tick.
    store
        .insert_instance_with_caps(
            "inst-1",
            "mock",
            Some(&run_id),
            None,
            Some(2.0),
            Utc::now() - Duration::hours(1),
            &InstanceCaps {
                max_lifetime_secs: None,
                max_cost_usd: Some(1.0),
                idle_timeout_secs: None,
            },
        )
        .unwrap();
    store.update_run_instance_id(&run_id, "inst-1").unwrap();
    let cap = Capture::default();
    let vendor = MockVendor::new(vec![Vec::new(); 3], vec![]);
    run_poller(&tmp, store, run_id.clone(), vendor, &cap);

    let kinds = cap.kinds();
    assert!(
        kinds.contains(&Kind::BudgetAutoDestroyed),
        "expected auto-destroy, got {kinds:?}"
    );
    // Warn thresholds crossed in the same tick still fire before the stop.
    assert!(kinds.contains(&Kind::BudgetWarn));
    let n = cap.find(Kind::BudgetAutoDestroyed).unwrap();
    assert!(n.title.contains("cost_cap"), "{}", n.title);
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    assert_eq!(
        store.get_run(&run_id).unwrap().unwrap().status,
        RunStatus::Failed
    );
}

#[test]
fn cleanup_failure_is_urgent_notification() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    let mut vendor = MockVendor::new(vec![lines(&[ev("train", "fail", Some("boom"))])], vec![]);
    vendor.destroy_fails = true;
    run_poller(&tmp, store, run_id, vendor, &cap);

    let n = cap
        .find(Kind::InstanceCleanupFailed)
        .expect("cleanup failure must notify");
    assert_eq!(n.priority, xrun_notify::Priority::Urgent);
    assert!(n.title.contains("inst-1"), "{}", n.title);
    assert!(n.body.contains("vendor said no"), "{}", n.body);
}

#[test]
fn events_filter_suppresses_unselected_kinds() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    let cfg = NotifyConfig {
        events: vec!["run.failed".into(), "budget.*".into()],
        ..NotifyConfig::default()
    };
    let nt = Notifier::with_channels(cfg, vec![Box::new(cap.clone())]);
    let vendor = MockVendor::new(vec![lines(&[ev("done", "ok", None)])], vec![]);
    let _ = Poller::new(
        run_id,
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(fast())
    .with_notifier(nt)
    .run(CancellationToken::new());
    assert!(cap.kinds().is_empty(), "run.done should be filtered out");
}

// ---------------------------------------------------------------------------
// Hot-reload: a channel configured on disk mid-run is picked up without a
// daemon restart. The webhook channel points at wiremock so we can assert
// the push actually left the process.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_settings_reload_mid_run_without_restart() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let cfg_dir = tmp.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // Start with notifications OFF on disk.
    std::fs::write(cfg_dir.join("config.toml"), "[notify]\nchannels = []\n").unwrap();

    let (store, run_id) = setup(&tmp);

    // Vendor that flips the on-disk config after its second tick, then
    // finishes the run two ticks later. Ticks are 0 s apart, but the reload
    // probe is rate-limited to 5 s, so the test sleeps past that window
    // once after writing.
    struct ReloadVendor {
        tick: AtomicUsize,
        cfg_dir: PathBuf,
        hook_url: String,
    }
    impl VendorAdapter for ReloadVendor {
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
        fn tail(&self, _: &InstanceHandle, file: &str, _: u64) -> Result<Vec<u8>, VendorError> {
            if !file.contains("events") {
                return Ok(Vec::new());
            }
            let t = self.tick.fetch_add(1, Ordering::SeqCst);
            if t == 1 {
                std::fs::write(
                    self.cfg_dir.join("config.toml"),
                    "[notify]\nchannels = [\"webhook\"]\n",
                )
                .unwrap();
                std::fs::write(
                    self.cfg_dir.join("credentials.toml"),
                    format!("[webhook]\nurl = \"{}\"\n", self.hook_url),
                )
                .unwrap();
                // Past the reload rate limit before the next tick probes.
                std::thread::sleep(std::time::Duration::from_millis(5200));
            }
            if t == 3 {
                return Ok(lines(&[ev("done", "ok", None)]));
            }
            Ok(Vec::new())
        }
        fn pull(&self, _: &InstanceHandle, _: &str, _: &Path) -> Result<(), VendorError> {
            Ok(())
        }
        fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
            Ok(())
        }
    }

    use std::path::PathBuf;
    let vendor = ReloadVendor {
        tick: AtomicUsize::new(0),
        cfg_dir: cfg_dir.clone(),
        hook_url: format!("{}/hook", server.uri()),
    };
    let global = xrun_core::GlobalConfig::load(&cfg_dir).unwrap();
    let (initial, _) = Notifier::from_config(&global.notify, &xrun_core::Credentials::default());
    assert!(initial.is_empty(), "test must start with notifications off");

    let status = Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(fast())
    .with_notifier(initial)
    .with_notify_reload(&cfg_dir)
    .run(CancellationToken::new())
    .unwrap();
    assert_eq!(status, RunStatus::Done);

    // wiremock's expect(1) is verified on drop; also check the journal.
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    let log = store.list_notify_log(None, 10).unwrap();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].kind, "run.done");
    assert_eq!(log[0].channel, "webhook");
    assert!(log[0].ok);
}

// ---------------------------------------------------------------------------
// Round 3: early-stop, monthly budget, hook.notify relay
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use xrun_core::manifest::{EarlyStop, EarlyStopMode};

/// Mock that records `pull` calls (the early-stop path pulls before destroy).
struct PullingVendor {
    inner: MockVendor,
    pulls: Arc<AtomicUsize>,
    last_pattern: Arc<Mutex<String>>,
}

impl VendorAdapter for PullingVendor {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn validate(&self, m: &Manifest) -> Result<(), VendorError> {
        self.inner.validate(m)
    }
    fn dry_run_plan(&self, m: &Manifest) -> Result<DryRunPlan, VendorError> {
        self.inner.dry_run_plan(m)
    }
    fn provision(&self, m: &Manifest) -> Result<InstanceHandle, VendorError> {
        self.inner.provision(m)
    }
    fn upload(&self, h: &InstanceHandle, d: &[DataSource]) -> Result<(), VendorError> {
        self.inner.upload(h, d)
    }
    fn execute(&self, h: &InstanceHandle, r: &RunSpec) -> Result<(), VendorError> {
        self.inner.execute(h, r)
    }
    fn tail(&self, h: &InstanceHandle, f: &str, o: u64) -> Result<Vec<u8>, VendorError> {
        self.inner.tail(h, f, o)
    }
    fn pull(&self, _: &InstanceHandle, pattern: &str, into: &Path) -> Result<(), VendorError> {
        self.pulls.fetch_add(1, AtomicOrdering::SeqCst);
        *self.last_pattern.lock().unwrap() = pattern.to_string();
        std::fs::write(into.join("best.pt"), b"weights").unwrap();
        Ok(())
    }
    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        self.inner.destroy(h)
    }
}

fn es_spec(metric: &str, patience: u32, mode: EarlyStopMode) -> EarlyStop {
    EarlyStop {
        metric: metric.into(),
        patience,
        mode,
        min_delta: 0.0,
        pull: true,
        pull_pattern: None,
    }
}

#[test]
fn early_stop_pulls_destroys_and_marks_done() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    // val_f1 improves twice, then flatlines for 3 evals → patience 3 fires
    // on the third stale point; the later `done` event must never be read.
    let metrics = vec![
        lines(&[metric("val_f1", 1, "0.70"), metric("val_f1", 2, "0.80")]),
        lines(&[metric("val_f1", 3, "0.79"), metric("val_f1", 4, "0.80")]),
        lines(&[metric("val_f1", 5, "0.78")]),
        lines(&[metric("val_f1", 6, "0.99")]), // never reached
    ];
    let pulls = Arc::new(AtomicUsize::new(0));
    let pattern = Arc::new(Mutex::new(String::new()));
    let vendor = PullingVendor {
        inner: MockVendor::new(vec![Vec::new(); 6], metrics),
        pulls: pulls.clone(),
        last_pattern: pattern.clone(),
    };
    let status = Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(fast())
    .with_early_stop(es_spec("val_f1", 3, EarlyStopMode::Max))
    .with_notifier(notifier(&cap))
    .run(CancellationToken::new())
    .unwrap();

    assert_eq!(status, RunStatus::Done);
    assert_eq!(pulls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(*pattern.lock().unwrap(), "**/best*");
    assert!(tmp
        .path()
        .join("runs")
        .join(run_id.to_string())
        .join("artifacts")
        .join("best.pt")
        .exists());

    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    assert_eq!(
        store.get_run(&run_id).unwrap().unwrap().status,
        RunStatus::Done
    );
    let ev = store.list_events(&run_id).unwrap();
    let es = ev
        .iter()
        .find(|e| e.stage == "early_stop")
        .expect("early_stop event");
    assert!(
        es.msg.as_deref().unwrap().contains("best 0.8000 at step 2"),
        "{:?}",
        es.msg
    );
    // Only the 5 stored points (step 6 never ingested).
    assert_eq!(cap.kinds(), vec![Kind::RunEarlyStopped]);
    let n = cap.find(Kind::RunEarlyStopped).unwrap();
    assert!(
        n.body.contains("best_pt") || n.body.contains("pulled to"),
        "{}",
        n.body
    );
}

#[test]
fn early_stop_min_mode_and_min_delta() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    // loss: 1.0, 0.95 (improves by 0.05 < min_delta 0.1 → stale), 0.94, 0.5 (big improvement resets),
    // 0.49, 0.48 → stale 2 → patience 2 fires at step 6.
    let metrics = vec![lines(&[
        metric("loss", 1, "1.0"),
        metric("loss", 2, "0.95"),
        metric("loss", 3, "0.94"),
    ])];
    let vendor = MockVendor::new(vec![Vec::new(); 3], metrics);
    let mut spec = es_spec("loss", 2, EarlyStopMode::Min);
    spec.min_delta = 0.1;
    spec.pull = false;
    let status = Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(fast())
    .with_early_stop(spec)
    .with_notifier(notifier(&cap))
    .run(CancellationToken::new())
    .unwrap();
    assert_eq!(status, RunStatus::Done);
    let n = cap.find(Kind::RunEarlyStopped).unwrap();
    assert!(
        n.body.contains("best loss = 1.0000 at step 1"),
        "{}",
        n.body
    );
    assert!(n.body.contains("not pulled"), "{}", n.body);
}

#[test]
fn early_stop_does_not_fire_while_improving() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    let metrics = vec![lines(&[
        metric("val_f1", 1, "0.1"),
        metric("val_f1", 2, "0.2"),
        metric("val_f1", 3, "0.3"),
        metric("val_f1", 4, "0.4"),
    ])];
    let vendor = MockVendor::new(vec![Vec::new(), lines(&[ev("done", "ok", None)])], metrics);
    let status = Poller::new(
        run_id,
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(fast())
    .with_early_stop(es_spec("val_f1", 2, EarlyStopMode::Max))
    .with_notifier(notifier(&cap))
    .run(CancellationToken::new())
    .unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(cap.kinds(), vec![Kind::RunDone]);
}

#[test]
fn monthly_budget_soft_alert_fires_once() {
    let tmp = TempDir::new().unwrap();
    let (mut store, run_id) = setup(&tmp);
    // $10/h instance, 1 h old → $10 month-to-date; cap $5.
    store
        .insert_instance_with_caps(
            "inst-1",
            "mock",
            Some(&run_id),
            None,
            Some(10.0),
            Utc::now() - Duration::hours(1),
            &InstanceCaps::default(),
        )
        .unwrap();
    store.update_run_instance_id(&run_id, "inst-1").unwrap();
    let cap = Capture::default();
    let vendor = MockVendor::new(
        vec![Vec::new(), Vec::new(), lines(&[ev("done", "ok", None)])],
        vec![],
    );
    let budget = xrun_core::config::BudgetConfig {
        monthly_budget_usd: Some(5.0),
        max_cost_per_instance_usd: 1000.0,
        ..Default::default()
    };
    let _ = Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(fast())
    .with_budget(budget)
    .with_notifier(notifier(&cap))
    .run(CancellationToken::new());

    let kinds = cap.kinds();
    assert_eq!(
        kinds.iter().filter(|k| **k == Kind::BudgetMonthly).count(),
        1,
        "{kinds:?}"
    );
    let store = Store::open(&tmp.path().join("runs.db")).unwrap();
    let ev = store.list_events(&run_id).unwrap();
    assert_eq!(
        ev.iter()
            .filter(|e| e.stage == "budget.monthly_exceeded")
            .count(),
        1
    );
}

#[test]
fn hook_notify_event_is_relayed_verbatim() {
    let tmp = TempDir::new().unwrap();
    let (store, run_id) = setup(&tmp);
    let cap = Capture::default();
    let ts = Utc::now().to_rfc3339();
    let note = format!(
        r#"{{"ts":"{ts}","stage":"notify","status":"ok","msg":"epoch 10","extra":{{"body":"val_f1=0.91","priority":"high"}}}}"#
    )
    .into_bytes();
    let vendor = MockVendor::new(vec![lines(&[note, ev("done", "ok", None)])], vec![]);
    run_poller(&tmp, store, run_id, vendor, &cap);
    assert_eq!(cap.kinds(), vec![Kind::User, Kind::RunDone]);
    let n = cap.find(Kind::User).unwrap();
    assert!(n.title.contains("epoch 10"), "{}", n.title);
    assert_eq!(n.body, "val_f1=0.91");
    assert_eq!(n.priority, xrun_notify::Priority::High);
}
