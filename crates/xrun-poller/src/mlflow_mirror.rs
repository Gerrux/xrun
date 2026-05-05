#![deny(unsafe_code)]

//! `MlflowMirror` — thin sync wrapper around a single `MetricSink`.
//!
//! v0.7 introduced the `MetricSink` trait so multiple tracking servers
//! (MLflow, WandB, …) can fan out from one poller. For backward compat the
//! mirror's public name + API are unchanged; internally it now delegates to
//! a `Box<dyn MetricSink>` (currently always `MlflowSink`). Slice 3 renames
//! this struct to `MetricFanOut` and grows it to a `Vec<Box<dyn MetricSink>>`.
//!
//! The `block()` helper exists because the surrounding `Poller` is sync —
//! it polls files on a 5s tick from a `std::thread`. `MetricSink` is async
//! (every backend is HTTP), so each call crosses the sync→async boundary
//! once. When called from inside an existing tokio runtime (kaggle adapter
//! does this) we use `block_in_place` + `Handle::block_on`; otherwise we
//! spin up a temporary single-thread runtime.

use std::collections::HashMap;

use xrun_core::metric_sink::{MetricPoint, MetricSink, OpenRunCtx, RemoteRunHandle};
use xrun_core::store::{NewMetric, RunId, RunStatus, Store};
use xrun_mlflow::{Auth, MlflowSink};

/// Block on a future, either by using the current tokio runtime handle (if one
/// exists) or by falling back to a temporary single-threaded runtime.
fn block<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");
            rt.block_on(fut)
        }
    }
}

/// Configuration for MLflow mirroring. Public name and field shape are
/// unchanged from v0.5 — slice 3 will broaden this to `MetricSinksConfig`
/// once a second sink (WandB) lands.
#[derive(Debug, Clone)]
pub struct MlflowMirrorConfig {
    pub url: String,
    pub experiment: String,
    pub auth: Option<Auth>,
    pub log_args_as_params: bool,
    pub run_name: Option<String>,
    pub vendor: String,
    pub instance_id: Option<String>,
}

/// Runtime state for the metric mirror — holds an opened `RemoteRunHandle`
/// and the underlying sink.
pub struct MlflowMirror {
    /// Opaque sink handle. `None` until `start()` succeeds. After a `start()`
    /// failure stays `None` and all subsequent calls become no-ops.
    handle: Option<RemoteRunHandle>,
    sink: Box<dyn MetricSink>,
    config: MlflowMirrorConfig,
    /// One-shot warn latch — surface the first failure, swallow the rest so
    /// the log doesn't drown in the same backend error every 5s.
    warned: bool,
}

impl MlflowMirror {
    pub fn new(config: MlflowMirrorConfig) -> Self {
        let sink = MlflowSink::new(
            config.url.clone(),
            config.auth.clone(),
            config.log_args_as_params,
        );
        Self {
            handle: None,
            sink: Box::new(sink),
            config,
            warned: false,
        }
    }

    /// Initialize the remote run. On failure: emit one warning and disable
    /// mirroring silently (handle stays `None`, all subsequent calls no-op).
    pub fn start(
        &mut self,
        run_id: &RunId,
        store: &mut Store,
        args: Option<&HashMap<String, serde_json::Value>>,
    ) {
        // OpenRunCtx borrows everything; build local owned scratch space and
        // hand out borrows. Empty maps are fine (sink-side: skips param
        // logging when the config map is empty).
        let empty_args: HashMap<String, serde_json::Value> = HashMap::new();
        let args_ref = args.unwrap_or(&empty_args);
        let tags: HashMap<String, String> = HashMap::new();

        let run_id_str = run_id.to_string();
        let ctx = OpenRunCtx {
            run_id: &run_id_str,
            experiment: &self.config.experiment,
            run_name: self.config.run_name.as_deref(),
            vendor: &self.config.vendor,
            instance_id: self.config.instance_id.as_deref(),
            config: args_ref,
            tags: &tags,
        };

        match block(self.sink.open_run(ctx)) {
            Ok(handle) => {
                if let Err(e) = set_mlflow_run_id(store, run_id, &handle.remote_run_id) {
                    tracing::warn!("could not persist mlflow_run_id: {e}");
                }
                tracing::info!(
                    sink = %self.sink.name(),
                    remote_run_id = %handle.remote_run_id,
                    "metric sink run started"
                );
                self.handle = Some(handle);
            }
            Err(e) => {
                tracing::warn!(
                    "{} sink open failed (mirroring disabled for this run): {e}",
                    self.sink.name()
                );
                self.warned = true;
            }
        }
    }

    /// Mirror a batch of metrics. Empty input is a no-op; on failure, warn
    /// once and silently skip subsequent batches.
    pub fn log_metrics(&mut self, metrics: &[NewMetric]) {
        if metrics.is_empty() {
            return;
        }
        let Some(handle) = self.handle.as_ref() else {
            return;
        };

        let batch: Vec<MetricPoint> = metrics
            .iter()
            .map(|m| MetricPoint {
                key: m.key.clone(),
                value: m.value,
                timestamp_ms: m.ts.timestamp_millis(),
                step: m.step,
            })
            .collect();

        if let Err(e) = block(self.sink.log_metrics_batch(handle, &batch)) {
            if !self.warned {
                tracing::warn!(
                    "{} log_metrics_batch failed (will not retry for this run): {e}",
                    self.sink.name()
                );
                self.warned = true;
            }
        }
    }

    /// Finalize the remote run. Best-effort — failures are logged but not
    /// bubbled (the local store is already authoritative).
    pub fn finish(&self, status: &RunStatus) {
        let Some(handle) = self.handle.as_ref() else {
            return;
        };
        match block(self.sink.finalize(handle, status.clone())) {
            Ok(()) => {
                tracing::info!(
                    sink = %self.sink.name(),
                    remote_run_id = %handle.remote_run_id,
                    "metric sink run finalized"
                );
            }
            Err(e) => {
                tracing::warn!("{} finalize failed: {e}", self.sink.name());
            }
        }
    }
}

/// Persist `mlflow_run_id` to the store.
fn set_mlflow_run_id(
    store: &mut Store,
    run_id: &RunId,
    mlflow_id: &str,
) -> Result<(), xrun_core::StoreError> {
    store.set_mlflow_run_id(run_id, mlflow_id)
}
