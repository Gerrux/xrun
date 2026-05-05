#![deny(unsafe_code)]

//! Pluggable metric/log sinks: MLflow, WandB, Comet, …
//!
//! A `MetricSink` is a fan-out target that mirrors xrun's authoritative local
//! state (SQLite + JSONL) to an external tracking server. The local store is
//! always source-of-truth — sinks fail loud once at start, then degrade silent
//! per call so a network blip on `wandb.ai` never breaks `xrun events`.
//!
//! Why the trait is async: every supported sink is HTTP-backed and the poller
//! already has a tokio runtime in scope (via `MlflowClient`). A sync trait
//! would force every impl to bring its own block_on, duplicating the
//! threading-quirks workaround `xrun-poller::metric_fanout::block` already
//! solves once. Sync callers (`xrun-poller`'s 5s tick loop) cross the boundary
//! through that helper.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use thiserror::Error;

use crate::store::RunStatus;

/// One metric sample. Mirrors `store::NewMetric` but flat, sink-agnostic, and
/// without lifetime ties to the local DB row — sinks can buffer/batch freely.
#[derive(Debug, Clone)]
pub struct MetricPoint {
    pub key: String,
    pub value: f64,
    /// Wall-clock millis since epoch, matching MLflow's `timestamp` semantics.
    /// Sinks that prefer relative seconds (WandB) compute it from `step` or
    /// the run's start time on their side.
    pub timestamp_ms: i64,
    pub step: i64,
}

/// Context handed to `open_run` — everything a remote tracking server needs
/// to bind the new run to the right experiment / project / hyperparams.
///
/// Borrowed so callers can build it from store rows + manifest views without
/// cloning. Sinks that need to store any of it (most do, for `finalize`)
/// pull it into their `RemoteRunHandle` payload.
pub struct OpenRunCtx<'a> {
    /// xrun's own run id (ULID). Use this as the correlation key — sinks
    /// should set `tags.xrun_run_id = run_id` so a tracking-server search
    /// round-trips back to the local row.
    pub run_id: &'a str,
    /// xrun manifest `name` field. Maps to MLflow `experiment_name`, WandB
    /// `project`, Comet `project_name`.
    pub experiment: &'a str,
    /// Optional human-readable run name. MLflow `mlflow.runName`,
    /// WandB `name`, Comet `experiment_name`.
    pub run_name: Option<&'a str>,
    /// `Vendor::as_str()` — `"vast"` / `"kaggle"` / `"local"` / `"ssh"`.
    /// Mirrored as a tag so the tracking UI can filter.
    pub vendor: &'a str,
    /// Adapter-allocated instance id (vast `19283`, kaggle slug, ssh alias…).
    pub instance_id: Option<&'a str>,
    /// Hyperparameters from `manifest.run.args`. MLflow logs as `params`,
    /// WandB merges into `config`, Comet as `parameters`. Empty map = none.
    pub config: &'a HashMap<String, serde_json::Value>,
    /// Free-form tags (vendor, instance_id are auto-added by the impl).
    pub tags: &'a HashMap<String, String>,
}

/// Sink-side handle to an opened run. Opaque to callers — only the sink that
/// minted it knows what `remote_run_id` means.
#[derive(Debug, Clone)]
pub struct RemoteRunHandle {
    /// Sink name (matches `MetricSink::name()`). Used for log lines and to
    /// route subsequent calls back to the right sink in a fan-out.
    pub sink_name: String,
    /// Sink-internal run id. MLflow run_uuid, WandB run id, Comet experiment
    /// key. Persisted to the xrun store so a poller restart can resume.
    pub remote_run_id: String,
    /// Web URL that opens the run in the tracking UI, when the sink can
    /// build one. Surfaced by `xrun show` / `xrun metrics --mlflow-url`.
    pub remote_url: Option<String>,
}

/// Errors a sink can return. Granular enough to retry on `Network` /
/// `Server`, fail loud on `Auth` / `Config`, swallow on `Disabled`.
#[derive(Debug, Error)]
pub enum MetricSinkError {
    /// Credentials missing or rejected by the server. Don't retry — surface
    /// to the user via `xrun doctor`.
    #[error("auth: {0}")]
    Auth(String),
    /// Sink is disabled (no api_key configured, opt-out flag set, …).
    /// Pollers swallow this silently — it's a normal "no sink wired" state.
    #[error("disabled: {0}")]
    Disabled(String),
    /// Transient HTTP / TLS error. Caller may retry with backoff.
    #[error("network: {0}")]
    Network(String),
    /// Server returned 5xx or malformed payload. May retry.
    #[error("server: {0}")]
    Server(String),
    /// Config is invalid (unknown sink name, malformed URL, …). Don't retry.
    #[error("config: {0}")]
    Config(String),
    /// Catch-all for sink-specific errors. Use the more specific variants
    /// when you can — most callers branch on the variant for log levels.
    #[error("{0}")]
    Other(String),
}

/// Pluggable mirror sink for metrics/events/artifacts.
///
/// Lifecycle: `open_run` once per xrun-run → many `log_metrics_batch` /
/// `log_artifact` → exactly one `finalize`. After `finalize`, the handle is
/// considered closed; further calls may 4xx on some servers (MLflow rejects
/// log-batch on a `FINISHED` run). Pollers keep the handle around only for
/// the duration of one run.
///
/// All methods are best-effort by contract: a sink that returns Err on
/// `log_metrics_batch` should still accept future batches — the caller logs
/// once and keeps going. Use `MetricSinkError::Auth` / `Config` if the
/// degradation is permanent (no point retrying), or `Network` / `Server`
/// for transient failures.
#[async_trait]
pub trait MetricSink: Send + Sync {
    /// Stable identifier — `"mlflow"`, `"wandb"`, `"comet"`. Matches the
    /// string used in `[metrics] sinks = […]` config and TUI screens.
    fn name(&self) -> &str;

    /// Open a remote run. Idempotency is the sink's job — most servers tag
    /// runs with `xrun_run_id` so a re-open of the same xrun-run finds the
    /// existing remote run instead of creating a duplicate.
    async fn open_run(&self, ctx: OpenRunCtx<'_>) -> Result<RemoteRunHandle, MetricSinkError>;

    /// Push a batch of metric points. Empty batch is a no-op (don't error).
    /// Implementations should chunk if their server caps batch size (MLflow:
    /// 1000 metrics per call) — caller passes whatever the local poller has.
    async fn log_metrics_batch(
        &self,
        handle: &RemoteRunHandle,
        batch: &[MetricPoint],
    ) -> Result<(), MetricSinkError>;

    /// Upload an artifact file (checkpoint, plot, log) under `name` in the
    /// run's artifact tree. `path` is local; sinks read it on their own
    /// thread. `name` may include forward-slash subpaths (`"plots/loss.png"`)
    /// where the server supports it.
    async fn log_artifact(
        &self,
        handle: &RemoteRunHandle,
        path: &Path,
        name: &str,
    ) -> Result<(), MetricSinkError>;

    /// Mark the run as terminal. `status` maps to the sink's enum:
    /// MLflow `FINISHED/FAILED/KILLED`, WandB `finished/failed/crashed`,
    /// Comet `finished/error`. Sinks should accept `Done` / `Failed` /
    /// `Cancelled` and treat anything else as best-effort `Done`.
    async fn finalize(
        &self,
        handle: &RemoteRunHandle,
        status: RunStatus,
    ) -> Result<(), MetricSinkError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_point_is_clone() {
        let p = MetricPoint {
            key: "loss".into(),
            value: 0.42,
            timestamp_ms: 1_700_000_000_000,
            step: 5,
        };
        let q = p.clone();
        assert_eq!(p.key, q.key);
        assert_eq!(p.step, q.step);
    }

    #[test]
    fn open_run_ctx_borrows() {
        // Compile-only: confirms callers can build OpenRunCtx from borrowed
        // pieces without cloning everything.
        let cfg: HashMap<String, serde_json::Value> = HashMap::new();
        let tags: HashMap<String, String> = HashMap::new();
        let _ctx = OpenRunCtx {
            run_id: "01ABCD",
            experiment: "exp",
            run_name: Some("run-1"),
            vendor: "vast",
            instance_id: None,
            config: &cfg,
            tags: &tags,
        };
    }
}
