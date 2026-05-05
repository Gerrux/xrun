#![deny(unsafe_code)]

//! `MetricFanOut` — fan out one xrun-run's events/metrics to N tracking
//! servers (MLflow, WandB, …) in parallel.
//!
//! Replaces v0.5's hardcoded `MlflowMirror`. Each enabled sink opens its own
//! remote run, gets its own `RemoteRunHandle`, and receives the same
//! metric-batch sequence. A single sink's failure (network, auth, server
//! 5xx) only suppresses *that* sink's mirror — siblings keep going, and
//! the local SQLite store stays authoritative regardless.
//!
//! The `block()` helper bridges sync→async exactly once. Pollers run on a
//! `std::thread`; sinks are async (every backend is HTTP). When already
//! inside a tokio runtime (kaggle adapter does this) we use
//! `block_in_place + Handle::block_on`; otherwise we spin up a transient
//! current-thread runtime per call.

use std::collections::HashMap;

use xrun_core::metric_sink::{MetricPoint, MetricSink, OpenRunCtx, RemoteRunHandle};
use xrun_core::store::{NewMetric, RunId, RunStatus, Store};
use xrun_mlflow::{Auth, MlflowSink};
use xrun_wandb::{WandbClient, WandbSink, DEFAULT_API_BASE};

/// Block on a future, either by using the current tokio runtime handle (if
/// one exists) or by falling back to a temporary single-threaded runtime.
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

/// MLflow sub-config — `None` disables the MLflow mirror for this run.
#[derive(Debug, Clone)]
pub struct MlflowSubConfig {
    pub url: String,
    pub auth: Option<Auth>,
    /// Mirror manifest hyperparameters as MLflow `params`. Off for sinks
    /// like WandB where `config` is the right place; on for MLflow because
    /// its UI keys off `params` for the run-comparison view.
    pub log_args_as_params: bool,
}

/// WandB sub-config — `None` disables the WandB mirror for this run.
#[derive(Debug, Clone)]
pub struct WandbSubConfig {
    pub api_key: String,
    /// Pin the wandb entity (team / username). When `None` the sink probes
    /// `viewer { entity }` once and caches the result.
    pub entity: Option<String>,
    /// Override the API base. Defaults to `https://api.wandb.ai`. Set for
    /// self-hosted Weights & Biases instances or wiremock tests.
    pub api_base: Option<String>,
    /// Override the public web base used to build `remote_url`. Defaults to
    /// `https://wandb.ai` (different host from the API).
    pub web_base: Option<String>,
}

/// Combined config for one fan-out instance. Each `Some` sub-config opens
/// one remote run; an empty config (all `None`) is a valid no-op fan-out.
#[derive(Debug, Clone, Default)]
pub struct MetricSinksConfig {
    /// MLflow experiment / project name. Same field is reused as wandb
    /// project — both backends key plots off this.
    pub experiment: String,
    /// Optional human-readable run name.
    pub run_name: Option<String>,
    /// Vendor name (`vast` / `kaggle` / `local` / `ssh`). Mirrored as a tag
    /// to every sink so the dashboards can filter.
    pub vendor: String,
    /// Adapter-allocated instance id, when known.
    pub instance_id: Option<String>,
    pub mlflow: Option<MlflowSubConfig>,
    pub wandb: Option<WandbSubConfig>,
}

impl MetricSinksConfig {
    pub fn is_empty(&self) -> bool {
        self.mlflow.is_none() && self.wandb.is_none()
    }
}

/// One enabled sink + its handle (after a successful `start`). The handle
/// is `None` while the sink is enabled-but-not-yet-opened, so a `start()`
/// failure on one sink doesn't poison the others.
struct ActiveSink {
    sink: Box<dyn MetricSink>,
    handle: Option<RemoteRunHandle>,
    /// One-shot warn latch per sink. Surface the first failure, swallow
    /// the rest so a 5xx storm doesn't drown the log every 5s.
    warned: bool,
}

impl ActiveSink {
    fn new(sink: Box<dyn MetricSink>) -> Self {
        Self {
            sink,
            handle: None,
            warned: false,
        }
    }
}

/// Fan-out mirror. Holds N `ActiveSink`s; `start()` opens runs on all of
/// them in sequence, `log_metrics()` fans batches out to all in parallel,
/// `finish()` finalizes all. A transient failure on one sink doesn't keep
/// siblings from getting metrics.
pub struct MetricFanOut {
    sinks: Vec<ActiveSink>,
    config: MetricSinksConfig,
}

impl MetricFanOut {
    pub fn new(config: MetricSinksConfig) -> Self {
        let mut sinks: Vec<ActiveSink> = Vec::new();

        if let Some(mlf) = config.mlflow.clone() {
            let s = MlflowSink::new(mlf.url.clone(), mlf.auth.clone(), mlf.log_args_as_params);
            sinks.push(ActiveSink::new(Box::new(s)));
        }
        if let Some(w) = config.wandb.clone() {
            let api_base = w
                .api_base
                .clone()
                .unwrap_or_else(|| DEFAULT_API_BASE.into());
            let mut sink = WandbSink::new(WandbClient::new(api_base, w.api_key), w.entity);
            if let Some(web) = w.web_base {
                sink = sink.with_web_base(web);
            }
            sinks.push(ActiveSink::new(Box::new(sink)));
        }

        Self { sinks, config }
    }

    /// True when no sinks are enabled — caller can skip `start`/`finish`
    /// to avoid pointless overhead.
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }

    /// Open remote runs on every enabled sink. Sequential by design — most
    /// runs only have 1-2 sinks, parallelizing the open path is not worth
    /// the complexity. Per-sink failures are logged and the sink stays
    /// disabled for the lifetime of this `MetricFanOut`.
    pub fn start(
        &mut self,
        run_id: &RunId,
        store: &mut Store,
        args: Option<&HashMap<String, serde_json::Value>>,
    ) {
        let empty_args: HashMap<String, serde_json::Value> = HashMap::new();
        let args_ref = args.unwrap_or(&empty_args);
        let tags: HashMap<String, String> = HashMap::new();
        let run_id_str = run_id.to_string();

        // Track the first MLflow run id we open so the kaggle adapter (which
        // queries by xrun_run_id tag) and `xrun metrics --mlflow-url` keep
        // working unchanged. WandB's bucket id is also useful but isn't
        // persisted yet — adding `wandb_run_id` to the schema is slice 4.
        let mut first_mlflow_id: Option<String> = None;

        for active in &mut self.sinks {
            let ctx = OpenRunCtx {
                run_id: &run_id_str,
                experiment: &self.config.experiment,
                run_name: self.config.run_name.as_deref(),
                vendor: &self.config.vendor,
                instance_id: self.config.instance_id.as_deref(),
                config: args_ref,
                tags: &tags,
            };
            match block(active.sink.open_run(ctx)) {
                Ok(handle) => {
                    tracing::info!(
                        sink = %active.sink.name(),
                        remote_run_id = %handle.remote_run_id,
                        "metric sink run opened"
                    );
                    if active.sink.name() == "mlflow" && first_mlflow_id.is_none() {
                        first_mlflow_id = Some(handle.remote_run_id.clone());
                    }
                    active.handle = Some(handle);
                }
                Err(e) => {
                    tracing::warn!(
                        "{} sink open failed (this sink disabled for the run): {e}",
                        active.sink.name()
                    );
                    active.warned = true;
                }
            }
        }

        if let Some(id) = first_mlflow_id {
            if let Err(e) = store.set_mlflow_run_id(run_id, &id) {
                tracing::warn!("could not persist mlflow_run_id: {e}");
            }
        }
    }

    /// Fan out a metric batch to every opened sink. Each sink's HTTP call
    /// runs concurrently within one `block` boundary, so total latency is
    /// `max(per_sink_latency)` instead of the sum.
    ///
    /// Per-sink failures are logged once (the `warned` latch) and future
    /// batches still attempt — a brief network blip on wandb won't
    /// permanently disable the mirror.
    pub fn log_metrics(&mut self, metrics: &[NewMetric]) {
        if metrics.is_empty() || self.sinks.is_empty() {
            return;
        }
        let batch: Vec<MetricPoint> = metrics
            .iter()
            .map(|m| MetricPoint {
                key: m.key.clone(),
                value: m.value,
                timestamp_ms: m.ts.timestamp_millis(),
                step: m.step,
            })
            .collect();

        let errors: Vec<(String, String)> = block(async {
            let mut futs = Vec::new();
            for active in self.sinks.iter() {
                if let Some(h) = active.handle.as_ref() {
                    let name = active.sink.name().to_string();
                    let fut = active.sink.log_metrics_batch(h, &batch);
                    futs.push(async move { (name, fut.await) });
                }
            }
            // Manual unordered drain — futures::join_all isn't a workspace
            // dep yet, and we typically have ≤2 sinks so the constant-
            // factor difference vs. tokio::spawn is negligible.
            let mut errs = Vec::new();
            for fut in futs {
                let (name, res) = fut.await;
                if let Err(e) = res {
                    errs.push((name, e.to_string()));
                }
            }
            errs
        });

        for (name, msg) in &errors {
            if let Some(active) = self.sinks.iter_mut().find(|a| a.sink.name() == name) {
                if !active.warned {
                    tracing::warn!(
                        "{} log_metrics_batch failed (subsequent failures suppressed): {}",
                        name,
                        msg
                    );
                    active.warned = true;
                }
            }
        }
    }

    /// Finalize every opened sink with the same status. Errors are logged
    /// but never surfaced — the local SQLite row is the source of truth.
    pub fn finish(&self, status: &RunStatus) {
        for active in &self.sinks {
            let Some(h) = active.handle.as_ref() else {
                continue;
            };
            match block(active.sink.finalize(h, status.clone())) {
                Ok(()) => tracing::info!(
                    sink = %active.sink.name(),
                    remote_run_id = %h.remote_run_id,
                    "metric sink run finalized"
                ),
                Err(e) => tracing::warn!("{} finalize failed: {e}", active.sink.name()),
            }
        }
    }
}
