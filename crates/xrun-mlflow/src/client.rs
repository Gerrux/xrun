#![deny(unsafe_code)]

use std::time::Duration;

use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use tracing::warn;

use crate::error::MlflowError;
use crate::types::{ExperimentId, MlflowMetric, MlflowParam, MlflowTag, RunId, RunStatus};

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_MS: u64 = 200;
const REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub enum Auth {
    Bearer(String),
    Basic { username: String, password: String },
}

#[derive(Debug, Clone)]
pub struct MlflowClient {
    base_url: String,
    auth: Option<Auth>,
    client: Client,
}

impl MlflowClient {
    pub fn new(base_url: impl Into<String>, auth: Option<Auth>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("failed to build HTTP client");
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            auth,
            client,
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "{}/api/2.0/mlflow/{}",
            self.base_url,
            path.trim_start_matches('/')
        )
    }

    fn apply_auth(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.auth {
            Some(Auth::Bearer(token)) => builder.bearer_auth(token),
            Some(Auth::Basic { username, password }) => {
                builder.basic_auth(username, Some(password))
            }
            None => builder,
        }
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, MlflowError> {
        let url = self.api_url(path);
        self.send_with_retry(|| self.apply_auth(self.client.get(&url).query(query)))
            .await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, MlflowError> {
        let url = self.api_url(path);
        self.send_with_retry(|| self.apply_auth(self.client.post(&url).json(&body)))
            .await
    }

    async fn send_with_retry<F>(&self, build: F) -> Result<Value, MlflowError>
    where
        F: Fn() -> RequestBuilder,
    {
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            let req = build();
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();

                    if status.is_success() {
                        let val: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);
                        return Ok(val);
                    }

                    // 4xx — don't retry
                    if status.is_client_error() {
                        return Err(map_client_error(status, body_text));
                    }

                    // 5xx — retry with backoff
                    warn!(
                        attempt,
                        status = status.as_u16(),
                        "MLflow request got 5xx, will retry"
                    );
                    last_err = Some(MlflowError::Internal {
                        status: status.as_u16(),
                        body: body_text,
                    });
                }
                Err(e) => {
                    warn!(attempt, error = %e, "MLflow request failed, will retry");
                    last_err = Some(MlflowError::Network(e));
                }
            }

            if attempt + 1 < MAX_RETRIES {
                let delay = RETRY_BASE_MS * (1 << attempt);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }

        Err(last_err.unwrap_or_else(|| MlflowError::Internal {
            status: 0,
            body: "unknown error after retries".to_string(),
        }))
    }

    pub async fn get_or_create_experiment(&self, name: &str) -> Result<ExperimentId, MlflowError> {
        // Try GET first
        match self
            .get("experiments/get-by-name", &[("experiment_name", name)])
            .await
        {
            Ok(val) => {
                let id = val["experiment"]["experiment_id"]
                    .as_str()
                    .ok_or_else(|| MlflowError::Parse("missing experiment_id in response".into()))?
                    .to_string();
                return Ok(id);
            }
            Err(MlflowError::NotFound(_)) => {
                // Fall through to create
            }
            Err(e) => return Err(e),
        }

        // Create experiment
        let resp = self
            .post("experiments/create", json!({ "name": name }))
            .await?;
        let id = resp["experiment_id"]
            .as_str()
            .ok_or_else(|| MlflowError::Parse("missing experiment_id in create response".into()))?
            .to_string();
        Ok(id)
    }

    pub async fn create_run(
        &self,
        experiment_id: &str,
        tags: &[MlflowTag],
    ) -> Result<RunId, MlflowError> {
        let ts = chrono::Utc::now().timestamp_millis();
        let body = json!({
            "experiment_id": experiment_id,
            "start_time": ts,
            "tags": tags,
        });
        let resp = self.post("runs/create", body).await?;
        let run_id = resp["run"]["info"]["run_id"]
            .as_str()
            .ok_or_else(|| MlflowError::Parse("missing run_id in create response".into()))?
            .to_string();
        Ok(run_id)
    }

    pub async fn update_run(
        &self,
        run_id: &str,
        status: RunStatus,
        end_time: Option<i64>,
    ) -> Result<(), MlflowError> {
        let mut body = json!({
            "run_id": run_id,
            "status": status.as_str(),
        });
        if let Some(ts) = end_time {
            body["end_time"] = json!(ts);
        }
        self.post("runs/update", body).await?;
        Ok(())
    }

    pub async fn log_param(&self, run_id: &str, key: &str, value: &str) -> Result<(), MlflowError> {
        self.post(
            "runs/log-parameter",
            json!({ "run_id": run_id, "key": key, "value": value }),
        )
        .await?;
        Ok(())
    }

    pub async fn log_metric(
        &self,
        run_id: &str,
        key: &str,
        value: f64,
        step: i64,
        timestamp: i64,
    ) -> Result<(), MlflowError> {
        self.post(
            "runs/log-metric",
            json!({
                "run_id": run_id,
                "key": key,
                "value": value,
                "step": step,
                "timestamp": timestamp,
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn log_batch(
        &self,
        run_id: &str,
        params: &[MlflowParam],
        metrics: &[MlflowMetric],
        tags: &[MlflowTag],
    ) -> Result<(), MlflowError> {
        let body = json!({
            "run_id": run_id,
            "params": params,
            "metrics": metrics,
            "tags": tags,
        });
        self.post("runs/log-batch", body).await?;
        Ok(())
    }

    /// Search runs in `experiment_id` whose tag `tag_key` matches `tag_value`.
    ///
    /// Returns the matching run_ids (most recent first per MLflow's default
    /// ordering). Used by the kaggle adapter to look up the streamer's run by
    /// the `xrun_run_id` tag — there's no other way to recover the MLflow
    /// run_id created in-kernel without a side channel.
    pub async fn search_runs_by_tag(
        &self,
        experiment_id: &str,
        tag_key: &str,
        tag_value: &str,
    ) -> Result<Vec<String>, MlflowError> {
        let body = json!({
            "experiment_ids": [experiment_id],
            "filter": format!("tags.{} = '{}'", tag_key, tag_value),
            "max_results": 100,
        });
        let resp = self.post("runs/search", body).await?;
        let mut ids = Vec::new();
        if let Some(arr) = resp.get("runs").and_then(|v| v.as_array()) {
            for run in arr {
                if let Some(id) = run
                    .get("info")
                    .and_then(|i| i.get("run_id"))
                    .and_then(|s| s.as_str())
                {
                    ids.push(id.to_string());
                }
            }
        }
        Ok(ids)
    }

    /// Fetch a run's `artifact_uri` and strip the `mlflow-artifacts:` scheme,
    /// returning the proxy-relative path prefix (e.g. `1/<run_id>/artifacts`).
    /// MLflow's artifact proxy keys storage off the URL path, not `?run_id=`
    /// (which it accepts but ignores), so callers must pass this prefix to
    /// `list_artifacts` / `download_artifact` to hit the right run's bucket.
    pub async fn get_run_artifact_path(&self, run_id: &str) -> Result<String, MlflowError> {
        let resp = self.get("runs/get", &[("run_id", run_id)]).await?;
        let uri = resp
            .get("run")
            .and_then(|r| r.get("info"))
            .and_then(|i| i.get("artifact_uri"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MlflowError::Parse("missing artifact_uri in runs/get".into()))?;
        let stripped = uri
            .splitn(2, ':')
            .nth(1)
            .unwrap_or(uri)
            .trim_start_matches('/');
        Ok(stripped.to_string())
    }

    /// List artifacts under `<base_path>/<sub_path>` (or just `<base_path>`
    /// when sub_path is None) on the proxy. Returns `(full_path_from_proxy_root,
    /// file_size_bytes)` for each non-directory entry.
    ///
    /// `base_path` is the run-scoped storage prefix from `get_run_artifact_path`.
    /// Uses MLflow's artifact-proxy endpoint (`mlflow-artifacts/artifacts`),
    /// which is the only listing path that works with `mlflow server
    /// --serve-artifacts`.
    pub async fn list_artifacts(
        &self,
        base_path: &str,
        sub_path: Option<&str>,
    ) -> Result<Vec<(String, u64)>, MlflowError> {
        let full_query_path = match sub_path {
            Some(s) if !s.is_empty() => format!("{}/{}", base_path.trim_end_matches('/'), s),
            _ => base_path.trim_end_matches('/').to_string(),
        };
        let url = format!("{}/api/2.0/mlflow-artifacts/artifacts", self.base_url);
        let builder = self
            .client
            .get(&url)
            .query(&[("path", full_query_path.as_str())]);
        let builder = self.apply_auth(builder);

        let resp = match builder.send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                return Err(map_client_error(status, body));
            }
            Err(e) => return Err(MlflowError::Network(e)),
        };
        let body_text = resp.text().await.map_err(MlflowError::Network)?;
        let parsed: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        // Re-prepend the queried sub_path so callers can re-use these paths
        // for follow-up downloads (full_query_path → relative result paths).
        let prefix = sub_path.map(|p| p.trim_end_matches('/'));
        let mut out = Vec::new();
        if let Some(files) = parsed.get("files").and_then(|v| v.as_array()) {
            for f in files {
                let is_dir = f.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false);
                if is_dir {
                    continue;
                }
                let raw_path = match f.get("path").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => continue,
                };
                let full = match prefix {
                    Some(pre) if !pre.is_empty() => format!("{pre}/{raw_path}"),
                    _ => raw_path.to_string(),
                };
                let size = f
                    .get("file_size")
                    .and_then(|v| v.as_u64())
                    .or_else(|| {
                        f.get("file_size")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<u64>().ok())
                    })
                    .unwrap_or(0);
                out.push((full, size));
            }
        }
        Ok(out)
    }

    /// Download an artifact via the MLflow artifact proxy. Returns raw bytes.
    /// `base_path` is the run-scoped prefix; `file_path` is the path under it
    /// (e.g. `logs/log_000001.txt`).
    pub async fn download_artifact(
        &self,
        base_path: &str,
        file_path: &str,
    ) -> Result<Vec<u8>, MlflowError> {
        let url = format!(
            "{}/api/2.0/mlflow-artifacts/artifacts/{}/{}",
            self.base_url,
            base_path.trim_matches('/'),
            file_path.trim_start_matches('/'),
        );
        let builder = self.apply_auth(self.client.get(&url));
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await.map_err(MlflowError::Network)?;
                Ok(bytes.to_vec())
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Err(map_client_error(status, body))
            }
            Err(e) => Err(MlflowError::Network(e)),
        }
    }

    /// Upload an artifact via the MLflow artifacts REST endpoint.
    /// For local MLflow (2.x), this uses PUT to the mlflow-artifacts path.
    pub async fn log_artifact(
        &self,
        run_id: &str,
        local_path: &std::path::Path,
        artifact_path: Option<&str>,
    ) -> Result<(), MlflowError> {
        let file_name = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| MlflowError::Parse("invalid local_path filename".into()))?;

        let remote_path = match artifact_path {
            Some(p) => format!("{}/{}", p.trim_end_matches('/'), file_name),
            None => file_name.to_string(),
        };

        let url = format!(
            "{}/api/2.0/mlflow-artifacts/artifacts/{}?run_id={}",
            self.base_url, remote_path, run_id
        );

        let bytes = std::fs::read(local_path)
            .map_err(|e| MlflowError::Parse(format!("failed to read artifact file: {e}")))?;

        let builder = self.client.put(&url).body(bytes);
        let builder = self.apply_auth(builder);

        match builder.send().await {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Err(map_client_error(status, body))
            }
            Err(e) => Err(MlflowError::Network(e)),
        }
    }
}

fn map_client_error(status: StatusCode, body: String) -> MlflowError {
    match status.as_u16() {
        401 => MlflowError::Auth,
        404 => MlflowError::NotFound(body),
        409 => MlflowError::Conflict(body),
        400 => MlflowError::BadRequest {
            status: status.as_u16(),
            body,
        },
        s if (400..500).contains(&s) => MlflowError::BadRequest { status: s, body },
        s => MlflowError::Internal { status: s, body },
    }
}
