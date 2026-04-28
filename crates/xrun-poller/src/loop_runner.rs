#![deny(unsafe_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use thiserror::Error;
use xrun_core::{
    store::{NewEvent, NewMetric, RunId, RunStatus, Store},
    vendor::{InstanceHandle, VendorAdapter},
    EventStatus, StoreError,
};

use crate::lock::{PollerLock, PollerLockError};
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
        }
    }

    pub fn with_config(mut self, config: PollerConfig) -> Self {
        self.config = config;
        self
    }

    pub fn run(mut self, cancel: CancellationToken) -> Result<RunStatus, PollerError> {
        let run_id_str = self.run_id.to_string();
        let pid_file = self.runs_dir.join(&run_id_str).join("poller.pid");
        let _lock = PollerLock::try_acquire(&run_id_str, pid_file)?;

        let mut offset_e = self
            .store
            .get_poll_offset(&self.run_id, &self.config.events_file)
            .unwrap_or(0);
        let mut offset_m = self
            .store
            .get_poll_offset(&self.run_id, &self.config.metrics_file)
            .unwrap_or(0);

        let mut last_progress = Instant::now();
        let mut last_offset_e = offset_e;

        loop {
            if cancel.is_cancelled() {
                let _ = self.vendor.destroy(&self.handle);
                self.store
                    .update_run_status(&self.run_id, RunStatus::Cancelled)?;
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
                            offset_e += delta;
                            let _ = self.store.update_poll_offset(
                                &self.run_id,
                                &self.config.events_file,
                                offset_e,
                            );
                            self.store
                                .update_run_status(&self.run_id, RunStatus::Done)?;
                            return Ok(RunStatus::Done);
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
                        return Ok(RunStatus::Failed);
                    }
                }
                Ok(_) => {}
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
                    for m in parse_metrics(&bytes) {
                        let _ = self.store.append_metric(
                            &self.run_id,
                            NewMetric {
                                step: m.step,
                                key: m.key.clone(),
                                value: m.value,
                                ts: m.ts,
                            },
                        );
                    }
                    offset_m += delta;
                    let _ = self.store.update_poll_offset(
                        &self.run_id,
                        &self.config.metrics_file,
                        offset_m,
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("tail metrics error: {e}");
                }
            }

            // --- cost estimate ---
            if let Ok(Some(run)) = self.store.get_run(&self.run_id) {
                if let Some(started_at) = run.started_at {
                    if let Ok(Some(inst)) = self.store.get_instance(&self.handle.id) {
                        if let Some(dph) = inst.price_per_hour {
                            let hours = (Utc::now() - started_at).num_seconds() as f64 / 3600.0;
                            let _ = self
                                .store
                                .update_run_cost_estimate(&self.run_id, hours * dph);
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
                    return Ok(RunStatus::Failed);
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
