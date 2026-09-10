#![deny(unsafe_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{NaiveDate, Utc};
use thiserror::Error;
use xrun_core::{
    budget,
    config::BudgetConfig,
    error::VendorError,
    manifest::{EarlyStop, EarlyStopMode},
    store::{NewEvent, NewMetric, RunId, RunStatus, Store},
    vendor::{InstanceHandle, VendorAdapter},
    Credentials, DataUpdate, EventStatus, GlobalConfig, StoreError,
};

use xrun_notify::{
    anomaly::AnomalyDetector,
    messages::{self, RunRef},
    Notification, Notifier, Priority,
};

use crate::lock::{PollerLock, PollerLockError};
use crate::metric_fanout::{MetricFanOut, MetricSinksConfig};
use crate::parser::{parse_events, parse_metrics, parse_stdout_metrics};

/// Lightweight cancellation primitive backed by an atomic flag.
#[derive(Debug, Clone)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimum spacing between two on-disk config probes for notify hot-reload.
const NOTIFY_RELOAD_MIN_INTERVAL: Duration = Duration::from_secs(5);

/// State for notification hot-reload: where to look and what we last saw.
struct NotifyReload {
    config_dir: PathBuf,
    /// (config.toml mtime, credentials.toml mtime); `UNIX_EPOCH` when the
    /// file is missing so "file created" also counts as a change.
    stamp: (SystemTime, SystemTime),
    last_check: Instant,
}

impl NotifyReload {
    fn mtime(path: &Path) -> SystemTime {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH)
    }

    fn stamp(dir: &Path) -> (SystemTime, SystemTime) {
        (
            Self::mtime(&dir.join("config.toml")),
            Self::mtime(&dir.join("credentials.toml")),
        )
    }
}

/// Policy applied when a `status=fail` event is received from the remote run.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FailPolicy {
    #[default]
    StopInstance,
    Keep,
    /// Treated as `StopInstance` in v0.1 with a warning.
    Reprovision,
}

/// Configuration for the polling loop.
pub struct PollerConfig {
    /// Seconds between polls when recent progress was observed.
    pub interval_active_secs: u64,
    /// Seconds between polls when no progress has been observed recently.
    pub interval_idle_secs: u64,
    /// Seconds without byte progress before switching to the idle interval.
    pub idle_threshold_secs: u64,
    /// Trigger a failure event if no progress occurs for this many minutes.
    /// `None` disables the idle timeout.
    pub on_idle_minutes: Option<u64>,
    /// Path to the events JSONL file on the remote instance.
    pub events_file: String,
    /// Path to the metrics JSONL file on the remote instance.
    pub metrics_file: String,
    /// Path to the stdout log file on the remote instance.
    pub stdout_file: String,
    /// Policy for handling `status=fail` events.
    pub on_stage_failed: FailPolicy,
}

impl Default for PollerConfig {
    fn default() -> Self {
        Self {
            interval_active_secs: 5,
            interval_idle_secs: 30,
            idle_threshold_secs: 60,
            on_idle_minutes: None,
            events_file: "/workspace/run/events.jsonl".to_string(),
            metrics_file: "/workspace/run/metrics.jsonl".to_string(),
            stdout_file: "/workspace/run/stdout.log".to_string(),
            on_stage_failed: FailPolicy::StopInstance,
        }
    }
}

#[derive(Debug, Error)]
pub enum PollerError {
    #[error("another poller is already active for this run")]
    AlreadyPolling,
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("vendor error: {0}")]
    Vendor(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<PollerLockError> for PollerError {
    fn from(e: PollerLockError) -> Self {
        match e {
            PollerLockError::AlreadyPolling => Self::AlreadyPolling,
            PollerLockError::Io(io) => Self::Io(io),
        }
    }
}

fn status_str(s: &EventStatus) -> &'static str {
    match s {
        EventStatus::Start => "start",
        EventStatus::Ok => "ok",
        EventStatus::Fail => "fail",
        EventStatus::Progress => "progress",
    }
}

/// Tails event and metric JSONL files from a running instance, stores results in the DB,
/// and returns when the run completes, fails, or is cancelled.
pub struct Poller {
    run_id: RunId,
    store: Store,
    vendor: Box<dyn VendorAdapter>,
    handle: InstanceHandle,
    config: PollerConfig,
    runs_dir: PathBuf,
    update_tx: Option<SyncSender<DataUpdate>>,
    budget: BudgetConfig,
    /// Last UTC date we emitted a daily-budget breach event for. Reset
    /// implicitly when the date rolls over.
    daily_alert_date: Option<NaiveDate>,
    /// Last UTC (year, month) we emitted a monthly-budget breach for.
    monthly_alert_month: Option<(i32, u32)>,
    /// Optional metric-sink fan-out config. Built from manifest + global
    /// config + credentials by the CLI launch path. `None` (or empty subs)
    /// means "local-only mirror" — SQLite is still the source of truth.
    sinks_config: Option<MetricSinksConfig>,
    /// Cached timestamp of the `train_start` event, set the first tick we
    /// observe one. Used as the idle-timer anchor when no metric activity has
    /// fired yet, so a long-but-still-progressing setup phase doesn't trip
    /// the idle cap. Stays `None` until the run actually starts training.
    train_started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Push-notification fan-out. `None` = notifications off (no channels
    /// configured). Every hook below is a no-op in that case.
    notifier: Option<Notifier>,
    /// Hot-reload source for notification settings. When set, the loop
    /// re-reads `config.toml` + `credentials.toml` whenever their mtime
    /// changes, so a channel configured in the TUI while a run is already
    /// training starts receiving pushes without restarting the daemon.
    notify_reload: Option<NotifyReload>,
    /// Cached run identity for message text. Filled lazily from the store.
    run_ref: Option<RunRef>,
    /// `budget.warn` thresholds (percent of --max-cost) already sent for
    /// this instance. In-memory latch; the notify_log dedupe covers daemon
    /// restarts.
    cost_warned: HashSet<u8>,
    anomalies: AnomalyDetector,
    /// `policy.early_stop` from the manifest, plus its running state.
    early_stop: Option<EarlyStopState>,
}

/// Running state for metric-based early stopping.
struct EarlyStopState {
    spec: EarlyStop,
    best: Option<f64>,
    best_step: i64,
    last_step: Option<i64>,
    stale: u32,
    /// Set once patience is exhausted; consumed by the loop.
    hit: bool,
}

impl EarlyStopState {
    fn new(spec: EarlyStop) -> Self {
        Self {
            spec,
            best: None,
            best_step: 0,
            last_step: None,
            stale: 0,
            hit: false,
        }
    }

    /// Feed a batch. Returns `true` the first time patience runs out.
    fn observe(&mut self, batch: &[NewMetric]) -> bool {
        if self.hit {
            return false;
        }
        let mut pts: Vec<(i64, f64)> = batch
            .iter()
            .filter(|m| m.key == self.spec.metric && m.value.is_finite())
            .map(|m| (m.step, m.value))
            .collect();
        if pts.is_empty() {
            return false;
        }
        pts.sort_by_key(|p| p.0);
        for (step, v) in pts {
            if self.last_step.is_some_and(|l| step <= l) {
                continue; // replay / duplicate step
            }
            self.last_step = Some(step);
            let improved = match self.best {
                None => true,
                Some(b) => match self.spec.mode {
                    EarlyStopMode::Max => v > b + self.spec.min_delta,
                    EarlyStopMode::Min => v < b - self.spec.min_delta,
                },
            };
            if improved {
                self.best = Some(v);
                self.best_step = step;
                self.stale = 0;
            } else {
                self.stale += 1;
                if self.stale >= self.spec.patience.max(1) {
                    self.hit = true;
                    return true;
                }
            }
        }
        false
    }
}

impl Poller {
    /// Do not report terminal cleanup success when the vendor rejected it.
    /// Keep the run active on error so resume/stop can retry the operation.
    fn destroy_instance(&mut self) -> Result<(), PollerError> {
        for attempt in 1..=3 {
            match self.vendor.destroy(&self.handle) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let message = format!("cleanup attempt {attempt}/3 failed: {error}");
                    tracing::warn!("{message}");
                    let _ = self.store.append_event(
                        &self.run_id,
                        NewEvent {
                            ts: Utc::now(),
                            stage: "instance.cleanup_failed".into(),
                            status: "fail".into(),
                            msg: Some(message.clone()),
                            payload_json: None,
                        },
                    );
                    if attempt == 3 {
                        let run_ref = self.run_ref();
                        let instance_id = self.handle.id.clone();
                        self.notify(messages::instance_cleanup_failed(
                            &run_ref,
                            &instance_id,
                            &message,
                        ));
                        return Err(PollerError::Vendor(message));
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        unreachable!()
    }
    pub fn new(
        run_id: RunId,
        store: Store,
        vendor: Box<dyn VendorAdapter>,
        handle: InstanceHandle,
        runs_dir: PathBuf,
    ) -> Self {
        Self {
            run_id,
            store,
            vendor,
            handle,
            config: PollerConfig::default(),
            runs_dir,
            update_tx: None,
            budget: BudgetConfig::default(),
            daily_alert_date: None,
            monthly_alert_month: None,
            sinks_config: None,
            train_started_at: None,
            notifier: None,
            notify_reload: None,
            run_ref: None,
            cost_warned: HashSet::new(),
            anomalies: AnomalyDetector::new(),
            early_stop: None,
        }
    }

    /// Enable metric-based early stopping (`policy.early_stop`).
    pub fn with_early_stop(mut self, spec: EarlyStop) -> Self {
        self.early_stop = Some(EarlyStopState::new(spec));
        self
    }

    /// Feed a metric batch to the early-stop evaluator (no-op when off).
    fn observe_early_stop(&mut self, batch: &[NewMetric]) {
        if let Some(es) = self.early_stop.as_mut() {
            es.observe(batch);
        }
    }

    /// `xrun_hook.notify(...)` events → push, verbatim. The script decides
    /// what matters; we only relay.
    fn relay_user_notes(
        &mut self,
        notes: Vec<(chrono::DateTime<Utc>, String, Option<String>, Priority)>,
    ) {
        if notes.is_empty() || self.notifier.is_none() {
            return;
        }
        let run_ref = self.run_ref();
        for (ts, title, body, prio) in notes {
            self.notify(messages::user_note(
                &run_ref,
                &title,
                body.as_deref(),
                prio,
                ts.timestamp_millis(),
            ));
        }
    }

    /// Wire push notifications. An empty notifier (no channels) is dropped
    /// so the hooks stay zero-cost.
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = if notifier.is_empty() {
            None
        } else {
            Some(notifier)
        };
        self
    }

    /// Watch `config_dir` and rebuild the notifier when the config or
    /// credentials file changes on disk. Checked once per tick, at most
    /// every `NOTIFY_RELOAD_MIN_INTERVAL`.
    pub fn with_notify_reload(mut self, config_dir: impl Into<PathBuf>) -> Self {
        let dir = config_dir.into();
        let stamp = NotifyReload::stamp(&dir);
        self.notify_reload = Some(NotifyReload {
            config_dir: dir,
            stamp,
            last_check: Instant::now(),
        });
        self
    }

    /// Cheap mtime probe; on change rebuild the notifier from disk. The
    /// in-memory latches (`cost_warned`, anomaly state) are kept — a config
    /// edit must not re-fire thresholds already announced.
    fn maybe_reload_notifier(&mut self) {
        let Some(r) = self.notify_reload.as_mut() else {
            return;
        };
        if r.last_check.elapsed() < NOTIFY_RELOAD_MIN_INTERVAL {
            return;
        }
        r.last_check = Instant::now();
        let now = NotifyReload::stamp(&r.config_dir);
        if now == r.stamp {
            return;
        }
        r.stamp = now;
        let global = GlobalConfig::load(&r.config_dir).unwrap_or_default();
        let creds = Credentials::load(&r.config_dir).unwrap_or_default();
        let (notifier, warnings) = Notifier::from_config(&global.notify, &creds);
        for w in warnings {
            tracing::warn!("notify reload: {w}");
        }
        let names = notifier.channel_names();
        tracing::info!(
            "notify: settings reloaded from {} (channels: {})",
            r.config_dir.display(),
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        );
        self.notifier = if notifier.is_empty() {
            None
        } else {
            Some(notifier)
        };
    }

    fn notify(&mut self, n: Notification) {
        if let Some(nt) = &self.notifier {
            nt.send(Some(&mut self.store), &n);
        }
    }

    fn run_ref(&mut self) -> RunRef {
        if let Some(r) = &self.run_ref {
            return r.clone();
        }
        let r = match self.store.get_run(&self.run_id) {
            Ok(Some(run)) => RunRef {
                id: run.id.to_string(),
                name: run.name,
                vendor: run.vendor,
                instance_id: run.instance_id.or_else(|| Some(self.handle.id.clone())),
            },
            _ => RunRef {
                id: self.run_id.to_string(),
                name: self.run_id.to_string(),
                vendor: self.vendor.name().to_string(),
                instance_id: Some(self.handle.id.clone()),
            },
        };
        self.run_ref = Some(r.clone());
        r
    }

    /// Best-known spend for this run: instance accumulated cost when the
    /// vendor bills per hour, else the run's cost estimate.
    fn cost_so_far(&self) -> Option<f64> {
        if let Ok(Some(inst)) = self.store.get_instance(&self.handle.id) {
            if inst.accumulated_cost > 0.0 {
                return Some(inst.accumulated_cost);
            }
        }
        self.store
            .get_run(&self.run_id)
            .ok()
            .flatten()
            .and_then(|r| r.cost_usd)
    }

    /// Terminal-status notification (`run.done` / `run.failed`). Cancelled
    /// is user-initiated and deliberately silent.
    fn notify_terminal(&mut self, status: &RunStatus, reason: Option<&str>) {
        if self.notifier.is_none() {
            return;
        }
        let run_ref = self.run_ref();
        let cost = self.cost_so_far();
        match status {
            RunStatus::Done => {
                let duration = self
                    .store
                    .get_run(&self.run_id)
                    .ok()
                    .flatten()
                    .and_then(|r| r.started_at)
                    .map(|s| (Utc::now() - s).num_seconds());
                self.notify(messages::run_done(&run_ref, duration, cost));
            }
            RunStatus::Failed => {
                let reason = reason.unwrap_or("see xrun events for the failing stage");
                self.notify(messages::run_failed(&run_ref, reason, cost));
            }
            _ => {}
        }
    }

    /// Run the anomaly rules over a freshly-ingested batch and notify once
    /// per tripped key.
    fn check_anomalies(&mut self, batch: &[NewMetric]) {
        if self.notifier.is_none() {
            return;
        }
        let mut hits = Vec::new();
        for m in batch {
            if let Some(a) = self.anomalies.observe(&m.key, Some(m.step), m.value) {
                hits.push(a);
            }
        }
        if hits.is_empty() {
            return;
        }
        let run_ref = self.run_ref();
        for a in hits {
            self.notify(messages::metric_anomaly(&run_ref, a.key(), &a.describe()));
        }
    }

    /// `budget.warn` at each configured percentage of `--max-cost`, once.
    fn check_cost_thresholds(&mut self, accumulated: f64, cap: Option<f64>) {
        let Some(nt) = &self.notifier else { return };
        let Some(cap) = cap.filter(|c| *c > 0.0) else {
            return;
        };
        let pct = accumulated / cap * 100.0;
        let due: Vec<u8> = nt
            .cost_warn_pct()
            .into_iter()
            .filter(|t| pct >= f64::from(*t) && !self.cost_warned.contains(t))
            .collect();
        if due.is_empty() {
            return;
        }
        let run_ref = self.run_ref();
        let instance_id = self.handle.id.clone();
        for t in due {
            self.cost_warned.insert(t);
            self.notify(messages::budget_warn(
                &run_ref,
                &instance_id,
                t,
                accumulated,
                cap,
            ));
        }
    }

    /// Wire the metric-sink fan-out. Pass empty config to disable all
    /// remote mirroring (local SQLite still authoritative).
    pub fn with_metric_sinks(mut self, config: MetricSinksConfig) -> Self {
        self.sinks_config = Some(config);
        self
    }

    pub fn with_config(mut self, config: PollerConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_budget(mut self, budget: BudgetConfig) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_update_sender(mut self, tx: SyncSender<DataUpdate>) -> Self {
        self.update_tx = Some(tx);
        self
    }

    fn send_update(&self, update: DataUpdate) {
        if let Some(tx) = &self.update_tx {
            let _ = tx.try_send(update);
        }
    }

    pub fn run(mut self, cancel: CancellationToken) -> Result<RunStatus, PollerError> {
        let run_id_str = self.run_id.to_string();
        let pid_file = self.runs_dir.join(&run_id_str).join("poller.pid");
        let _lock = PollerLock::try_acquire(&run_id_str, pid_file)?;

        // Initialize the metric-sink fan-out if configured. `MetricFanOut`
        // handles its own async runtime internally via block_in_place /
        // fallback runtime, and silently no-ops when no sinks are enabled.
        let mut mlflow: Option<MetricFanOut> = self.sinks_config.take().map(|cfg| {
            let mut fanout = MetricFanOut::new(cfg);
            fanout.start(&self.run_id, &mut self.store, None);
            fanout
        });

        let mut offset_e = self
            .store
            .get_poll_offset(&self.run_id, &self.config.events_file)
            .unwrap_or(0);
        let mut offset_m = self
            .store
            .get_poll_offset(&self.run_id, &self.config.metrics_file)
            .unwrap_or(0);
        let mut offset_s: u64 = 0;

        // Hydrate train_started_at from already-stored events so a restarted
        // daemon (e.g. after `cargo install --force` replaced xrun.exe on
        // Windows) doesn't lose the idle anchor.
        if self.train_started_at.is_none() {
            if let Ok(events) = self.store.list_events(&self.run_id) {
                self.train_started_at = events
                    .iter()
                    .find(|e| e.stage == "train_start")
                    .map(|e| e.ts);
            }
        }

        let mut last_progress = Instant::now();
        let mut last_offset_e = offset_e;
        // Holds the trailing partial line from the previous stdout chunk so
        // metrics aren't dropped on chunk boundaries (line boundaries don't
        // align with HTTP/MLflow chunk boundaries). Bounded by `MAX_STDOUT_LINE`
        // to prevent unbounded growth on a stream with no newlines.
        let mut stdout_line_buf: Vec<u8> = Vec::new();
        const MAX_STDOUT_LINE: usize = 64 * 1024;

        loop {
            let mut progress_this_tick = false;
            // Liveness stamp for `xrun watchdog`. Best-effort: a failed
            // UPDATE must never stall ingestion.
            let _ = self.store.update_run_heartbeat(&self.run_id, Utc::now());
            self.maybe_reload_notifier();
            // First `status=fail` event seen this tick — becomes the body of
            // the `run.failed` notification.
            let mut fail_reason: Option<String> = None;
            // `stage == "notify"` events authored by the training script.
            let mut user_notes: Vec<(chrono::DateTime<Utc>, String, Option<String>, Priority)> =
                Vec::new();
            if cancel.is_cancelled() {
                self.destroy_instance()?;
                self.store
                    .update_run_status(&self.run_id, RunStatus::Cancelled)?;
                self.send_update(DataUpdate::RunStatusChanged(
                    self.run_id.clone(),
                    RunStatus::Cancelled,
                ));
                if let Some(ref mirror) = mlflow {
                    mirror.finish(&RunStatus::Cancelled);
                }
                return Ok(RunStatus::Cancelled);
            }

            // Terminal status latched from the events tail. We do NOT return
            // immediately — we still drain metrics + stdout for this tick so
            // a fast run that wrote `done:ok` and the final metric in the
            // same window doesn't lose the metric. For vendors that destroy
            // remote state on failure (vast, kaggle) the drain runs before
            // destroy, so the remote files are still alive.
            let mut terminal_after_drain: Option<RunStatus> = None;
            let mut destroy_after_drain = false;

            // --- tail events ---
            match self
                .vendor
                .tail(&self.handle, &self.config.events_file, offset_e)
            {
                Ok(bytes) if !bytes.is_empty() => {
                    let delta = bytes.len() as u64;
                    let events = parse_events(&bytes);
                    let mut done = false;
                    let mut failed = false;

                    for ev in &events {
                        let payload = ev.extra.as_ref().map(|v| v.to_string());
                        let _ = self.store.append_event(
                            &self.run_id,
                            NewEvent {
                                ts: ev.ts,
                                stage: ev.stage.clone(),
                                status: status_str(&ev.status).to_string(),
                                msg: ev.msg.clone(),
                                payload_json: payload,
                            },
                        );

                        if ev.stage == "done" && ev.status == EventStatus::Ok {
                            done = true;
                        }

                        if ev.stage == "notify" {
                            let extra = ev.extra.as_ref();
                            let body = extra
                                .and_then(|e| e.get("body"))
                                .and_then(|b| b.as_str())
                                .map(str::to_string);
                            let prio = extra
                                .and_then(|e| e.get("priority"))
                                .and_then(|p| p.as_str())
                                .and_then(Priority::parse)
                                .unwrap_or(Priority::Default);
                            user_notes.push((
                                ev.ts,
                                ev.msg.clone().unwrap_or_else(|| "notify".into()),
                                body,
                                prio,
                            ));
                        }

                        if ev.status == EventStatus::Fail {
                            failed = true;
                            if fail_reason.is_none() {
                                fail_reason = Some(match &ev.msg {
                                    Some(m) => format!("{}: {m}", ev.stage),
                                    None => ev.stage.clone(),
                                });
                            }
                        }

                        // Anchor for the idle-timer fallback (see
                        // budget::idle_anchor): once training has started,
                        // the idle clock counts from there until the first
                        // heartbeat lands.
                        if self.train_started_at.is_none() && ev.stage == "train_start" {
                            self.train_started_at = Some(ev.ts);
                        }
                    }

                    offset_e += delta;
                    let _ = self.store.update_poll_offset(
                        &self.run_id,
                        &self.config.events_file,
                        offset_e,
                    );

                    if offset_e > last_offset_e {
                        last_progress = Instant::now();
                        last_offset_e = offset_e;
                        progress_this_tick = true;
                    }

                    self.send_update(DataUpdate::EventsAppended(
                        self.run_id.clone(),
                        events.len(),
                    ));
                    self.relay_user_notes(std::mem::take(&mut user_notes));

                    if done {
                        terminal_after_drain = Some(RunStatus::Done);
                    }

                    if failed && !matches!(self.config.on_stage_failed, FailPolicy::Keep) {
                        if matches!(self.config.on_stage_failed, FailPolicy::Reprovision) {
                            tracing::warn!(
                                "reprovision not supported in v0.1; treating as stop_instance"
                            );
                        }
                        terminal_after_drain = Some(RunStatus::Failed);
                        destroy_after_drain = true;
                    }
                }
                Ok(_) => {}
                Err(VendorError::Truncated) => {
                    tracing::warn!("events file truncated (pre-emption?); resetting offset to 0");
                    offset_e = 0;
                    let _ =
                        self.store
                            .update_poll_offset(&self.run_id, &self.config.events_file, 0);
                }
                Err(e) => {
                    tracing::warn!("tail events error: {e}");
                }
            }

            // --- tail metrics ---
            match self
                .vendor
                .tail(&self.handle, &self.config.metrics_file, offset_m)
            {
                Ok(bytes) if !bytes.is_empty() => {
                    let delta = bytes.len() as u64;
                    let metrics = parse_metrics(&bytes);
                    // Non-finite values (NaN loss) are recovered by the
                    // parser so the anomaly check can see them, but never
                    // persisted: SQLite stores NaN as NULL and the reader
                    // would choke on it later.
                    let all_metrics: Vec<NewMetric> = metrics
                        .iter()
                        .map(|m| NewMetric {
                            step: m.step,
                            key: m.key.clone(),
                            value: m.value,
                            ts: m.ts,
                        })
                        .collect();
                    self.check_anomalies(&all_metrics);
                    self.observe_early_stop(&all_metrics);
                    let new_metrics: Vec<NewMetric> = all_metrics
                        .into_iter()
                        .filter(|m| m.value.is_finite())
                        .collect();
                    let metrics_count = new_metrics.len();
                    for nm in &new_metrics {
                        let _ = self.store.append_metric(
                            &self.run_id,
                            NewMetric {
                                step: nm.step,
                                key: nm.key.clone(),
                                value: nm.value,
                                ts: nm.ts,
                            },
                        );
                    }
                    // Mirror to MLflow (silent degrade on error)
                    if let Some(ref mut mirror) = mlflow {
                        mirror.log_metrics(&new_metrics);
                    }
                    offset_m += delta;
                    let _ = self.store.update_poll_offset(
                        &self.run_id,
                        &self.config.metrics_file,
                        offset_m,
                    );
                    last_progress = Instant::now();
                    progress_this_tick = true;
                    self.send_update(DataUpdate::MetricsAppended(
                        self.run_id.clone(),
                        metrics_count,
                    ));
                }
                Ok(_) => {}
                Err(VendorError::Truncated) => {
                    tracing::warn!("metrics file truncated (pre-emption?); resetting offset to 0");
                    offset_m = 0;
                    let _ =
                        self.store
                            .update_poll_offset(&self.run_id, &self.config.metrics_file, 0);
                }
                Err(e) => {
                    tracing::warn!("tail metrics error: {e}");
                }
            }

            // --- snapshot stdout.log ---
            match self
                .vendor
                .tail(&self.handle, &self.config.stdout_file, offset_s)
            {
                Ok(bytes) if !bytes.is_empty() => {
                    let log_path = self
                        .runs_dir
                        .join(self.run_id.to_string())
                        .join("stdout.log");
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        let _ = f.write_all(&bytes);
                    }
                    offset_s += bytes.len() as u64;

                    // Best-effort: extract `key=value` / JSONL metrics from
                    // structured stdout. Canonical path is xrun_hook →
                    // metrics.jsonl, but for vendors where the hook isn't
                    // loaded (or for users who haven't adopted it), this
                    // gives `xrun metrics --ascii` something to draw without
                    // any manifest-side declaration. INSERT OR REPLACE on
                    // (run_id, key, step) makes this safe to overlap with
                    // the metrics.jsonl path.
                    let mut new_stdout_metrics: Vec<NewMetric> = Vec::new();
                    stdout_line_buf.extend_from_slice(&bytes);
                    let now_ts = Utc::now();
                    let mut consumed = 0;
                    while let Some(nl) =
                        stdout_line_buf[consumed..].iter().position(|&b| b == b'\n')
                    {
                        let end = consumed + nl;
                        let line = &stdout_line_buf[consumed..end];
                        for m in parse_stdout_metrics(line, now_ts) {
                            new_stdout_metrics.push(NewMetric {
                                step: m.step,
                                key: m.key,
                                value: m.value,
                                ts: m.ts,
                            });
                        }
                        consumed = end + 1;
                    }
                    stdout_line_buf.drain(..consumed);
                    if stdout_line_buf.len() > MAX_STDOUT_LINE {
                        // Pathological no-newline stream: drop the buffer
                        // rather than grow forever. Caller can still inspect
                        // stdout.log for the raw bytes.
                        stdout_line_buf.clear();
                    }
                    if !new_stdout_metrics.is_empty() {
                        let metrics_count = new_stdout_metrics.len();
                        for nm in &new_stdout_metrics {
                            let _ = self.store.append_metric(
                                &self.run_id,
                                NewMetric {
                                    step: nm.step,
                                    key: nm.key.clone(),
                                    value: nm.value,
                                    ts: nm.ts,
                                },
                            );
                        }
                        if let Some(ref mut mirror) = mlflow {
                            mirror.log_metrics(&new_stdout_metrics);
                        }
                        self.check_anomalies(&new_stdout_metrics);
                        self.observe_early_stop(&new_stdout_metrics);
                        self.send_update(DataUpdate::MetricsAppended(
                            self.run_id.clone(),
                            metrics_count,
                        ));
                        last_progress = Instant::now();
                        progress_this_tick = true;
                    }
                }
                Ok(_) => {}
                Err(VendorError::Truncated) => {
                    // Remote log was truncated (pre-emption restart): start over.
                    if let Ok(()) = std::fs::remove_file(
                        self.runs_dir
                            .join(self.run_id.to_string())
                            .join("stdout.log"),
                    ) {}
                    offset_s = 0;
                    stdout_line_buf.clear();
                }
                Err(e) => {
                    tracing::warn!("tail stdout error: {e}");
                }
            }

            // --- metric early-stop (policy.early_stop) ---
            if terminal_after_drain.is_none() && self.early_stop.as_ref().is_some_and(|es| es.hit) {
                let (spec, best, best_step) = {
                    let es = self.early_stop.as_ref().expect("checked above");
                    (es.spec.clone(), es.best.unwrap_or(f64::NAN), es.best_step)
                };
                let pattern = spec
                    .pull_pattern
                    .clone()
                    .unwrap_or_else(|| "**/best*".to_string());
                let run_dir = self.runs_dir.join(self.run_id.to_string());
                let mut pulled: Option<String> = None;
                if spec.pull {
                    let into = run_dir.join("artifacts");
                    let _ = std::fs::create_dir_all(&into);
                    match self.vendor.pull(&self.handle, &pattern, &into) {
                        Ok(()) => pulled = Some(into.display().to_string()),
                        Err(e) => tracing::warn!("early-stop: pull `{pattern}` failed: {e}"),
                    }
                }
                let payload = serde_json::json!({
                    "metric": spec.metric,
                    "best": best,
                    "best_step": best_step,
                    "patience": spec.patience,
                    "pulled_to": pulled,
                })
                .to_string();
                let _ = self.store.append_event(
                    &self.run_id,
                    NewEvent {
                        ts: Utc::now(),
                        stage: "early_stop".to_string(),
                        status: "ok".to_string(),
                        msg: Some(format!(
                            "{} plateaued: best {best:.4} at step {best_step}, \
                             no improvement for {} evals",
                            spec.metric, spec.patience
                        )),
                        payload_json: Some(payload),
                    },
                );
                self.destroy_instance()?;
                self.store
                    .update_run_status(&self.run_id, RunStatus::Done)?;
                self.send_update(DataUpdate::RunStatusChanged(
                    self.run_id.clone(),
                    RunStatus::Done,
                ));
                if let Some(ref mirror) = mlflow {
                    mirror.finish(&RunStatus::Done);
                }
                let run_ref = self.run_ref();
                let cost = self.cost_so_far();
                self.notify(messages::run_early_stopped(
                    &run_ref,
                    &spec.metric,
                    best,
                    best_step,
                    spec.patience,
                    pulled.as_deref(),
                    cost,
                ));
                return Ok(RunStatus::Done);
            }

            // PID liveness probe — fills the gap when bash + sshd are still
            // alive but the python child got SIGKILL'd (host OOM, etc).
            // Only meaningful after `train_start` has fired; before that the
            // run is still provisioning and the PID file may legitimately
            // not exist yet.
            if terminal_after_drain.is_none() && self.train_started_at.is_some() {
                if let Some(false) = self.vendor.process_alive(&self.handle) {
                    let _ = self.store.append_event(
                        &self.run_id,
                        NewEvent {
                            ts: Utc::now(),
                            stage: "stage_failed".to_string(),
                            status: "fail".to_string(),
                            msg: Some(
                                "training PID is gone but no done:ok was emitted \
                                 (likely OOM kill or unhandled exception)"
                                    .to_string(),
                            ),
                            payload_json: None,
                        },
                    );
                    terminal_after_drain = Some(RunStatus::Failed);
                    if fail_reason.is_none() {
                        fail_reason = Some(
                            "training PID gone without done:ok (OOM kill or unhandled exception?)"
                                .to_string(),
                        );
                    }
                    if !matches!(self.config.on_stage_failed, FailPolicy::Keep) {
                        destroy_after_drain = true;
                    }
                }
            }

            // Terminal status latched from events tail this tick: drain has
            // completed, finalize and return.
            if let Some(status) = terminal_after_drain {
                if destroy_after_drain {
                    self.destroy_instance()?;
                }
                self.store.update_run_status(&self.run_id, status.clone())?;
                self.send_update(DataUpdate::RunStatusChanged(
                    self.run_id.clone(),
                    status.clone(),
                ));
                if let Some(ref mirror) = mlflow {
                    mirror.finish(&status);
                }
                self.notify_terminal(&status, fail_reason.as_deref());
                return Ok(status);
            }

            // --- cost estimate + budget enforcement ---
            let now_wall = Utc::now();
            if let Ok(Some(run)) = self.store.get_run(&self.run_id) {
                if let Some(started_at) = run.started_at {
                    if let Ok(Some(inst)) = self.store.get_instance(&self.handle.id) {
                        if let Some(dph) = inst.price_per_hour {
                            let hours =
                                (now_wall - started_at).num_seconds().max(0) as f64 / 3600.0;
                            let _ = self
                                .store
                                .update_run_cost_estimate(&self.run_id, hours * dph);
                        }

                        // Refresh accumulated_cost from created_at (Vast bills
                        // from allocation, which is earlier than started_at).
                        let acc = budget::accumulate_cost(&inst, now_wall);
                        let active_ts = if progress_this_tick {
                            Some(now_wall)
                        } else {
                            None
                        };
                        let _ = self
                            .store
                            .update_instance_usage(&self.handle.id, acc, active_ts);

                        // Re-read with updated fields so cap evaluation uses
                        // the latest accumulated_cost / last_active_at.
                        if let Ok(Some(updated)) = self.store.get_instance(&self.handle.id) {
                            if updated.auto_destroyed_reason.is_none() {
                                self.check_cost_thresholds(acc, updated.max_cost_usd);
                                if let Some(reason) =
                                    budget::evaluate_caps(&updated, self.train_started_at, now_wall)
                                {
                                    self.destroy_instance()?;
                                    // Record success only after the vendor confirms cleanup.
                                    let _ = self.store.set_auto_destroyed_reason(
                                        &self.handle.id,
                                        reason.as_str(),
                                    );
                                    let payload = serde_json::json!({
                                        "reason": reason.as_str(),
                                        "instance_id": self.handle.id,
                                        "accumulated_cost": acc,
                                    })
                                    .to_string();
                                    let _ = self.store.append_event(
                                        &self.run_id,
                                        NewEvent {
                                            ts: now_wall,
                                            stage: "instance.auto_destroyed".to_string(),
                                            status: "fail".to_string(),
                                            msg: Some(format!(
                                                "auto-destroyed by budget guard: {}",
                                                reason.as_str()
                                            )),
                                            payload_json: Some(payload),
                                        },
                                    );
                                    self.store
                                        .update_run_status(&self.run_id, RunStatus::Failed)?;
                                    self.send_update(DataUpdate::RunStatusChanged(
                                        self.run_id.clone(),
                                        RunStatus::Failed,
                                    ));
                                    if let Some(ref mirror) = mlflow {
                                        mirror.finish(&RunStatus::Failed);
                                    }
                                    let run_ref = self.run_ref();
                                    let instance_id = self.handle.id.clone();
                                    self.notify(messages::budget_auto_destroyed(
                                        &run_ref,
                                        &instance_id,
                                        reason.as_str(),
                                        acc,
                                    ));
                                    return Ok(RunStatus::Failed);
                                }
                            }
                        }
                    }
                }
            }

            // --- daily budget soft-alert ---
            if let Some(daily_cap) = self.budget.daily_budget_usd {
                let today = budget::today_utc(now_wall);
                let already_alerted = self.daily_alert_date == Some(today);
                if !already_alerted {
                    if let Ok(spent) = budget::daily_spend(&self.store, today, now_wall) {
                        if spent >= daily_cap {
                            let payload = serde_json::json!({
                                "spent_usd": spent,
                                "limit_usd": daily_cap,
                                "date": today.to_string(),
                            })
                            .to_string();
                            let _ = self.store.append_event(
                                &self.run_id,
                                NewEvent {
                                    ts: now_wall,
                                    stage: "budget.daily_exceeded".to_string(),
                                    status: "fail".to_string(),
                                    msg: Some(format!(
                                        "daily spend ${:.2} >= cap ${:.2}",
                                        spent, daily_cap
                                    )),
                                    payload_json: Some(payload),
                                },
                            );
                            self.daily_alert_date = Some(today);
                            let run_ref = self.run_ref();
                            self.notify(messages::budget_daily(
                                &run_ref,
                                spent,
                                daily_cap,
                                self.budget.daily_budget_hard,
                            ));
                            // Hard daily stop is opt-in: it kills *this* run's
                            // instance (the daemon only sees one run). The
                            // user can also flip to soft mode by leaving
                            // `daily_budget_hard = false`.
                            if self.budget.daily_budget_hard {
                                self.destroy_instance()?;
                                let _ = self.store.set_auto_destroyed_reason(
                                    &self.handle.id,
                                    "daily_budget_hard",
                                );
                                self.store
                                    .update_run_status(&self.run_id, RunStatus::Failed)?;
                                self.send_update(DataUpdate::RunStatusChanged(
                                    self.run_id.clone(),
                                    RunStatus::Failed,
                                ));
                                if let Some(ref mirror) = mlflow {
                                    mirror.finish(&RunStatus::Failed);
                                }
                                return Ok(RunStatus::Failed);
                            }
                        }
                    }
                }
            }

            // --- monthly budget soft-alert ---
            if let Some(monthly_cap) = self.budget.monthly_budget_usd {
                let ym = budget::month_utc(now_wall);
                if self.monthly_alert_month != Some(ym) {
                    if let Ok(spent) = budget::monthly_spend(&self.store, ym.0, ym.1, now_wall) {
                        if spent >= monthly_cap {
                            let payload = serde_json::json!({
                                "spent_usd": spent,
                                "limit_usd": monthly_cap,
                                "month": format!("{}-{:02}", ym.0, ym.1),
                            })
                            .to_string();
                            let _ = self.store.append_event(
                                &self.run_id,
                                NewEvent {
                                    ts: now_wall,
                                    stage: "budget.monthly_exceeded".to_string(),
                                    status: "fail".to_string(),
                                    msg: Some(format!(
                                        "month-to-date spend ${:.2} >= cap ${:.2}",
                                        spent, monthly_cap
                                    )),
                                    payload_json: Some(payload),
                                },
                            );
                            self.monthly_alert_month = Some(ym);
                            let run_ref = self.run_ref();
                            self.notify(messages::budget_monthly(&run_ref, spent, monthly_cap));
                        }
                    }
                }
            }

            // --- idle detection ---
            let elapsed = last_progress.elapsed().as_secs();
            if let Some(idle_minutes) = self.config.on_idle_minutes {
                if elapsed > idle_minutes * 60 {
                    let _ = self.store.append_event(
                        &self.run_id,
                        NewEvent {
                            ts: Utc::now(),
                            stage: "idle".to_string(),
                            status: "fail".to_string(),
                            msg: Some(format!("no progress for {elapsed}s")),
                            payload_json: None,
                        },
                    );
                    self.destroy_instance()?;
                    self.store
                        .update_run_status(&self.run_id, RunStatus::Failed)?;
                    self.send_update(DataUpdate::RunStatusChanged(
                        self.run_id.clone(),
                        RunStatus::Failed,
                    ));
                    if let Some(ref mirror) = mlflow {
                        mirror.finish(&RunStatus::Failed);
                    }
                    let run_ref = self.run_ref();
                    let cost = self.cost_so_far();
                    self.notify(messages::run_idle(&run_ref, elapsed, cost));
                    return Ok(RunStatus::Failed);
                }
            }

            // --- vendor completion poll (Kaggle and similar non-streaming vendors) ---
            let run_dir = self.runs_dir.join(self.run_id.to_string());
            if let Some(completion) = self.vendor.poll_completion(&self.handle, &run_dir) {
                if !completion.events.is_empty() {
                    for ev in &completion.events {
                        let _ = self.store.append_event(
                            &self.run_id,
                            NewEvent {
                                ts: Utc::now(),
                                stage: ev.stage.clone(),
                                status: ev.status.clone(),
                                msg: ev.msg.clone(),
                                payload_json: None,
                            },
                        );
                    }
                    self.send_update(DataUpdate::EventsAppended(
                        self.run_id.clone(),
                        completion.events.len(),
                    ));
                }
                if let Some(terminal) = completion.terminal_status {
                    self.store
                        .update_run_status(&self.run_id, terminal.clone())?;
                    self.send_update(DataUpdate::RunStatusChanged(
                        self.run_id.clone(),
                        terminal.clone(),
                    ));
                    if let Some(ref mirror) = mlflow {
                        mirror.finish(&terminal);
                    }
                    let reason = completion
                        .events
                        .iter()
                        .find(|e| e.status == "fail")
                        .map(|e| match &e.msg {
                            Some(m) => format!("{}: {m}", e.stage),
                            None => e.stage.clone(),
                        });
                    self.notify_terminal(&terminal, reason.as_deref());
                    return Ok(terminal);
                }
            }

            let interval = if elapsed > self.config.idle_threshold_secs {
                self.config.interval_idle_secs
            } else {
                self.config.interval_active_secs
            };

            if interval > 0 {
                std::thread::sleep(Duration::from_secs(interval));
            }
        }
    }
}
