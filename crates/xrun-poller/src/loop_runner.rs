#![deny(unsafe_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, Utc};
use thiserror::Error;
use xrun_core::{
    budget,
    config::BudgetConfig,
    error::VendorError,
    store::{NewEvent, NewMetric, RunId, RunStatus, Store},
    vendor::{InstanceHandle, VendorAdapter},
    DataUpdate, EventStatus, StoreError,
};

use crate::lock::{PollerLock, PollerLockError};
use crate::mlflow_mirror::{MlflowMirror, MlflowMirrorConfig};
use crate::parser::{parse_events, parse_metrics};

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
    /// Optional MLflow mirror config. Set if both manifest.mlflow.experiment
    /// and GlobalConfig.mlflow.url are present.
    mlflow_config: Option<MlflowMirrorConfig>,
}

impl Poller {
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
            mlflow_config: None,
        }
    }

    pub fn with_mlflow(mut self, config: MlflowMirrorConfig) -> Self {
        self.mlflow_config = Some(config);
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

        // Initialize MLflow mirror if configured. MlflowMirror handles its
        // own async runtime internally via block_in_place / fallback runtime.
        let mut mlflow: Option<MlflowMirror> = self.mlflow_config.take().map(|cfg| {
            let mut mirror = MlflowMirror::new(cfg);
            mirror.start(&self.run_id, &mut self.store, None);
            mirror
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

        let mut last_progress = Instant::now();
        let mut last_offset_e = offset_e;

        loop {
            let mut progress_this_tick = false;
            if cancel.is_cancelled() {
                let _ = self.vendor.destroy(&self.handle);
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

                        if ev.status == EventStatus::Fail {
                            failed = true;
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

                    if done {
                        self.store
                            .update_run_status(&self.run_id, RunStatus::Done)?;
                        self.send_update(DataUpdate::RunStatusChanged(
                            self.run_id.clone(),
                            RunStatus::Done,
                        ));
                        if let Some(ref mirror) = mlflow {
                            mirror.finish(&RunStatus::Done);
                        }
                        return Ok(RunStatus::Done);
                    }

                    if failed && !matches!(self.config.on_stage_failed, FailPolicy::Keep) {
                        if matches!(self.config.on_stage_failed, FailPolicy::Reprovision) {
                            tracing::warn!(
                                "reprovision not supported in v0.1; treating as stop_instance"
                            );
                        }
                        let _ = self.vendor.destroy(&self.handle);
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
                    let metrics_count = metrics.len();
                    let mut new_metrics: Vec<NewMetric> = Vec::with_capacity(metrics.len());
                    for m in &metrics {
                        new_metrics.push(NewMetric {
                            step: m.step,
                            key: m.key.clone(),
                            value: m.value,
                            ts: m.ts,
                        });
                    }
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
                    let log_path = self.runs_dir.join(self.run_id.to_string()).join("stdout.log");
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        let _ = f.write_all(&bytes);
                    }
                    offset_s += bytes.len() as u64;
                }
                Ok(_) => {}
                Err(VendorError::Truncated) => {
                    // Remote log was truncated (pre-emption restart): start over.
                    if let Ok(()) = std::fs::remove_file(
                        self.runs_dir.join(self.run_id.to_string()).join("stdout.log"),
                    ) {}
                    offset_s = 0;
                }
                Err(e) => {
                    tracing::warn!("tail stdout error: {e}");
                }
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
                                if let Some(reason) = budget::evaluate_caps(&updated, now_wall) {
                                    // Record reason BEFORE destroying so a
                                    // daemon restart doesn't double-destroy.
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
                                    let _ = self.vendor.destroy(&self.handle);
                                    self.store
                                        .update_run_status(&self.run_id, RunStatus::Failed)?;
                                    self.send_update(DataUpdate::RunStatusChanged(
                                        self.run_id.clone(),
                                        RunStatus::Failed,
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
                            // Hard daily stop is opt-in: it kills *this* run's
                            // instance (the daemon only sees one run). The
                            // user can also flip to soft mode by leaving
                            // `daily_budget_hard = false`.
                            if self.budget.daily_budget_hard {
                                let _ = self.store.set_auto_destroyed_reason(
                                    &self.handle.id,
                                    "daily_budget_hard",
                                );
                                let _ = self.vendor.destroy(&self.handle);
                                self.store
                                    .update_run_status(&self.run_id, RunStatus::Failed)?;
                                self.send_update(DataUpdate::RunStatusChanged(
                                    self.run_id.clone(),
                                    RunStatus::Failed,
                                ));
                                return Ok(RunStatus::Failed);
                            }
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
                    let _ = self.vendor.destroy(&self.handle);
                    self.store
                        .update_run_status(&self.run_id, RunStatus::Failed)?;
                    self.send_update(DataUpdate::RunStatusChanged(
                        self.run_id.clone(),
                        RunStatus::Failed,
                    ));
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
                    self.store.update_run_status(&self.run_id, terminal.clone())?;
                    self.send_update(DataUpdate::RunStatusChanged(
                        self.run_id.clone(),
                        terminal.clone(),
                    ));
                    if let Some(ref mirror) = mlflow {
                        mirror.finish(&terminal);
                    }
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
