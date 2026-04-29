#![deny(unsafe_code)]

use std::collections::HashMap;

use chrono::Utc;
use xrun_core::store::{NewMetric, RunId, RunStatus, Store};
use xrun_mlflow::{Auth, MlflowClient, MlflowMetric, MlflowTag, RunStatus as MlflowRunStatus};

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

/// Configuration for MLflow mirroring.
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

/// Runtime state for the MLflow mirror — holds the MLflow run_id and client.
pub struct MlflowMirror {
    pub mlflow_run_id: Option<String>,
    client: MlflowClient,
    config: MlflowMirrorConfig,
    /// Track whether we've already emitted a warning for this session,
    /// so subsequent errors are silently skipped.
    warned: bool,
}

impl MlflowMirror {
    pub fn new(config: MlflowMirrorConfig) -> Self {
        let client = MlflowClient::new(&config.url, config.auth.clone());
        Self {
            mlflow_run_id: None,
            client,
            config,
            warned: false,
        }
    }

    /// Initialize the MLflow run: get/create experiment and create a run.
    /// On failure: emit one warning and disable mirroring silently.
    pub fn start(
        &mut self,
        run_id: &RunId,
        store: &mut Store,
        args: Option<&HashMap<String, serde_json::Value>>,
    ) {
        match block(self.do_start(args)) {
            Ok(mlflow_run_id) => {
                self.mlflow_run_id = Some(mlflow_run_id.clone());
                if let Err(e) = set_mlflow_run_id(store, run_id, &mlflow_run_id) {
                    tracing::warn!("could not persist mlflow_run_id: {e}");
                }
                tracing::info!(mlflow_run_id = %mlflow_run_id, "MLflow run started");
            }
            Err(e) => {
                tracing::warn!("MLflow start failed (mirroring disabled for this run): {e}");
                self.warned = true;
            }
        }
    }

    async fn do_start(
        &self,
        args: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<String, xrun_mlflow::MlflowError> {
        let exp_id = self
            .client
            .get_or_create_experiment(&self.config.experiment)
            .await?;

        let mut tags = vec![MlflowTag {
            key: "vendor".to_string(),
            value: self.config.vendor.clone(),
        }];
        if let Some(ref inst) = self.config.instance_id {
            tags.push(MlflowTag {
                key: "instance_id".to_string(),
                value: inst.clone(),
            });
        }
        if let Some(ref name) = self.config.run_name {
            tags.push(MlflowTag {
                key: "mlflow.runName".to_string(),
                value: name.clone(),
            });
        }

        let run_id = self.client.create_run(&exp_id, &tags).await?;

        if self.config.log_args_as_params {
            if let Some(args_map) = args {
                for (k, v) in args_map {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    // Best-effort: ignore individual param errors
                    let _ = self.client.log_param(&run_id, k, &val).await;
                }
            }
        }

        Ok(run_id)
    }

    /// Mirror a batch of metrics to MLflow.
    /// On failure: warn once, then silently skip.
    pub fn log_metrics(&mut self, metrics: &[NewMetric]) {
        let Some(run_id) = self.mlflow_run_id.as_deref() else {
            return;
        };

        let mlflow_metrics: Vec<MlflowMetric> = metrics
            .iter()
            .map(|m| MlflowMetric {
                key: m.key.clone(),
                value: m.value,
                timestamp: m.ts.timestamp_millis(),
                step: m.step,
            })
            .collect();

        if mlflow_metrics.is_empty() {
            return;
        }

        let run_id = run_id.to_string();
        match block(self.client.log_batch(&run_id, &[], &mlflow_metrics, &[])) {
            Ok(()) => {}
            Err(e) => {
                if !self.warned {
                    tracing::warn!("MLflow log_batch failed (will not retry for this run): {e}");
                    self.warned = true;
                }
            }
        }
    }

    /// Finalize the MLflow run with the given status.
    pub fn finish(&self, status: &RunStatus) {
        let Some(run_id) = self.mlflow_run_id.as_deref() else {
            return;
        };

        let mlflow_status = match status {
            RunStatus::Done => MlflowRunStatus::Finished,
            RunStatus::Failed => MlflowRunStatus::Failed,
            RunStatus::Cancelled => MlflowRunStatus::Killed,
            _ => MlflowRunStatus::Finished,
        };

        let end_time = Utc::now().timestamp_millis();
        let run_id = run_id.to_string();
        match block(
            self.client
                .update_run(&run_id, mlflow_status, Some(end_time)),
        ) {
            Ok(()) => {
                tracing::info!(mlflow_run_id = %run_id, "MLflow run finalized");
            }
            Err(e) => {
                tracing::warn!("MLflow update_run failed: {e}");
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
