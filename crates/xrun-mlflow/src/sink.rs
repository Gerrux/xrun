#![deny(unsafe_code)]

//! `MetricSink` impl backed by `MlflowClient`.
//!
//! Thin adapter — all real HTTP work lives in `client::MlflowClient`. This
//! file maps `xrun-core` types (`OpenRunCtx`, `MetricPoint`, `RunStatus`)
//! to MLflow's REST shape (`MlflowTag`, `MlflowMetric`, `RunStatus`) and
//! routes errors back through `MetricSinkError`.
//!
//! The sink is constructed with a base URL, optional auth, and a default
//! experiment name. `open_run` translates `OpenRunCtx.experiment` to the
//! same name (so the manifest's `name` field always wins over the configured
//! default) and creates / reuses the experiment.

use async_trait::async_trait;
use std::path::Path;
use xrun_core::metric_sink::{
    MetricPoint, MetricSink, MetricSinkError, OpenRunCtx, RemoteRunHandle,
};
use xrun_core::store::RunStatus as XrunStatus;

use crate::client::{Auth, MlflowClient};
use crate::error::MlflowError;
use crate::types::{MlflowMetric, MlflowParam, MlflowTag, RunStatus as MlflowStatus};

/// Map an MlflowError to the trait-level `MetricSinkError`. Auth and Parse
/// surface as user-actionable; everything 5xx-ish stays Network/Server so
/// callers can retry on backoff.
fn map_err(e: MlflowError) -> MetricSinkError {
    match e {
        MlflowError::Auth => MetricSinkError::Auth("MLflow rejected credentials (401)".into()),
        MlflowError::Network(inner) => MetricSinkError::Network(inner.to_string()),
        MlflowError::BadRequest { status, body } => {
            MetricSinkError::Server(format!("MLflow bad request (HTTP {status}): {body}"))
        }
        MlflowError::NotFound(b) => MetricSinkError::Server(format!("MLflow 404: {b}")),
        MlflowError::Conflict(b) => MetricSinkError::Server(format!("MLflow 409: {b}")),
        MlflowError::Internal { status, body } => {
            MetricSinkError::Server(format!("MLflow internal (HTTP {status}): {body}"))
        }
        MlflowError::Unexpected { status, body } => {
            MetricSinkError::Server(format!("MLflow unexpected (HTTP {status}): {body}"))
        }
        MlflowError::Parse(s) => MetricSinkError::Server(format!("MLflow parse: {s}")),
    }
}

#[derive(Clone)]
pub struct MlflowSink {
    base_url: String,
    client: MlflowClient,
    /// Whether the manifest's hyperparameters are mirrored as MLflow params.
    /// Disabled = treat them like WandB does (`config` only) and skip the
    /// per-key POST round-trips.
    log_args_as_params: bool,
}

impl MlflowSink {
    pub fn new(base_url: impl Into<String>, auth: Option<Auth>, log_args_as_params: bool) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let client = MlflowClient::new(&base_url, auth);
        Self {
            base_url,
            client,
            log_args_as_params,
        }
    }

    fn run_url(&self, experiment_id: &str, run_id: &str) -> String {
        format!(
            "{}/#/experiments/{}/runs/{}",
            self.base_url, experiment_id, run_id
        )
    }
}

#[async_trait]
impl MetricSink for MlflowSink {
    fn name(&self) -> &str {
        "mlflow"
    }

    async fn open_run(&self, ctx: OpenRunCtx<'_>) -> Result<RemoteRunHandle, MetricSinkError> {
        let exp_id = self
            .client
            .get_or_create_experiment(ctx.experiment)
            .await
            .map_err(map_err)?;

        // Build tags: caller's free-form tags plus the auto-added vendor /
        // instance / xrun_run_id markers so a tracking-server search can
        // round-trip back to the local run.
        let mut tags: Vec<MlflowTag> = ctx
            .tags
            .iter()
            .map(|(k, v)| MlflowTag {
                key: k.clone(),
                value: v.clone(),
            })
            .collect();
        tags.push(MlflowTag {
            key: "vendor".into(),
            value: ctx.vendor.into(),
        });
        if let Some(inst) = ctx.instance_id {
            tags.push(MlflowTag {
                key: "instance_id".into(),
                value: inst.into(),
            });
        }
        tags.push(MlflowTag {
            key: "xrun_run_id".into(),
            value: ctx.run_id.into(),
        });
        if let Some(name) = ctx.run_name {
            tags.push(MlflowTag {
                key: "mlflow.runName".into(),
                value: name.into(),
            });
        }

        let mlflow_run_id = self
            .client
            .create_run(&exp_id, &tags)
            .await
            .map_err(map_err)?;

        // Best-effort param mirroring. Individual failures are swallowed so
        // a single malformed value doesn't abort the open — params are nice-
        // to-have, not authoritative.
        if self.log_args_as_params {
            for (k, v) in ctx.config {
                let val = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let _ = self.client.log_param(&mlflow_run_id, k, &val).await;
            }
        }

        Ok(RemoteRunHandle {
            sink_name: "mlflow".into(),
            remote_run_id: mlflow_run_id.clone(),
            remote_url: Some(self.run_url(&exp_id, &mlflow_run_id)),
        })
    }

    async fn log_metrics_batch(
        &self,
        handle: &RemoteRunHandle,
        batch: &[MetricPoint],
    ) -> Result<(), MetricSinkError> {
        if batch.is_empty() {
            return Ok(());
        }
        let metrics: Vec<MlflowMetric> = batch
            .iter()
            .map(|m| MlflowMetric {
                key: m.key.clone(),
                value: m.value,
                timestamp: m.timestamp_ms,
                step: m.step,
            })
            .collect();

        // MLflow caps log-batch at 1000 metrics. Chunk so callers can pass
        // arbitrary-sized batches without surfacing the cap.
        const CHUNK: usize = 1000;
        let no_params: [MlflowParam; 0] = [];
        let no_tags: [MlflowTag; 0] = [];
        for chunk in metrics.chunks(CHUNK) {
            self.client
                .log_batch(&handle.remote_run_id, &no_params, chunk, &no_tags)
                .await
                .map_err(map_err)?;
        }
        Ok(())
    }

    async fn log_artifact(
        &self,
        handle: &RemoteRunHandle,
        path: &Path,
        name: &str,
    ) -> Result<(), MetricSinkError> {
        // `name` may include subpath segments — pass the dir part as
        // `artifact_path` and let MLflow's PUT use the file's basename.
        let (sub, _) = match name.rsplit_once('/') {
            Some((dir, file)) => (Some(dir), file),
            None => (None, name),
        };
        // Build a temp path matching `name`'s basename so MLflow honors the
        // intended on-server filename (its PUT uses `local_path.file_name()`).
        // Caller's `path` already points to the file we want to upload — we
        // ignore the `name` basename and rely on `path.file_name()`. Sub-path
        // (`plots/loss.png`) honored via `artifact_path`.
        self.client
            .log_artifact(&handle.remote_run_id, path, sub)
            .await
            .map_err(map_err)
    }

    async fn finalize(
        &self,
        handle: &RemoteRunHandle,
        status: XrunStatus,
    ) -> Result<(), MetricSinkError> {
        let mlflow_status = match status {
            XrunStatus::Done => MlflowStatus::Finished,
            XrunStatus::Failed => MlflowStatus::Failed,
            XrunStatus::Cancelled => MlflowStatus::Killed,
            // Defensive: anything still mid-flight on finalize means the
            // poller is shutting down without a clear terminal — record
            // FINISHED so the MLflow UI doesn't show a perpetual RUNNING.
            _ => MlflowStatus::Finished,
        };
        let end_time = chrono::Utc::now().timestamp_millis();
        self.client
            .update_run(&handle.remote_run_id, mlflow_status, Some(end_time))
            .await
            .map_err(map_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_url_builds_against_trailing_slash_base() {
        let s = MlflowSink::new("http://mlflow.example.com/", None, false);
        let url = s.run_url("12", "deadbeef");
        assert_eq!(
            url,
            "http://mlflow.example.com/#/experiments/12/runs/deadbeef"
        );
    }

    #[test]
    fn run_url_builds_against_plain_base() {
        let s = MlflowSink::new("http://mlflow.example.com", None, false);
        let url = s.run_url("12", "deadbeef");
        assert_eq!(
            url,
            "http://mlflow.example.com/#/experiments/12/runs/deadbeef"
        );
    }

    #[test]
    fn map_err_classifies_auth_separately() {
        let e = map_err(MlflowError::Auth);
        assert!(matches!(e, MetricSinkError::Auth(_)));
    }

    #[test]
    fn map_err_passes_through_5xx_as_server() {
        let e = map_err(MlflowError::Internal {
            status: 500,
            body: "boom".into(),
        });
        assert!(matches!(e, MetricSinkError::Server(_)));
    }
}
