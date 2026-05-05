#![deny(unsafe_code)]

//! Thin REST + GraphQL client for `api.wandb.ai`.
//!
//! WandB's public OpenAPI surface is small — most of its server logic is
//! exposed via the same internal endpoints the official `wandb` Python SDK
//! talks to. We use three of them:
//!
//! - `POST /graphql` — `viewer { entity }` for default-entity probes and
//!   `mutation upsertBucket` to create / reuse a run row.
//! - `POST /files/{entity}/{project}/{run}/file_stream` — append rows to
//!   `wandb-history.jsonl` (metrics) and signal `complete + exitcode` on
//!   finalize. This is the canonical streaming path; the SDK uses it too.
//!
//! Authentication is HTTP Basic with username `"api"` and the personal API
//! key as password. Bearer tokens aren't accepted for the file_stream path
//! at the time of writing.

use std::time::Duration;

use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use tracing::warn;

use crate::error::WandbError;
use crate::types::{ExitCode, HistoryLine, WandbRunInfo};

const MAX_RETRIES: u32 = 3;
const RETRY_BASE_MS: u64 = 200;
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Default WandB API host. The web UI lives at `https://wandb.ai`; API
/// lives at `https://api.wandb.ai`. They are different hosts and the
/// distinction matters for `web_url` link building.
pub const DEFAULT_API_BASE: &str = "https://api.wandb.ai";
pub const DEFAULT_WEB_BASE: &str = "https://wandb.ai";

#[derive(Debug, Clone)]
pub struct WandbClient {
    api_base: String,
    api_key: String,
    client: Client,
}

impl WandbClient {
    pub fn new(api_base: impl Into<String>, api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("failed to build HTTP client");
        Self {
            api_base: api_base.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client,
        }
    }

    fn graphql_url(&self) -> String {
        format!("{}/graphql", self.api_base)
    }

    fn file_stream_url(&self, entity: &str, project: &str, run_name: &str) -> String {
        format!(
            "{}/files/{}/{}/{}/file_stream",
            self.api_base, entity, project, run_name
        )
    }

    fn apply_auth(&self, builder: RequestBuilder) -> RequestBuilder {
        builder.basic_auth("api", Some(&self.api_key))
    }

    /// POST a JSON body with retry on transient (5xx, network) failures.
    /// 4xx surfaces immediately — wandb's body usually has actionable info.
    async fn post_json(&self, url: &str, body: &Value) -> Result<Value, WandbError> {
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            let req = self.apply_auth(self.client.post(url).json(body));
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let val: Value = resp
                            .json()
                            .await
                            .map_err(|e| WandbError::Parse(format!("response not JSON: {e}")))?;
                        return Ok(val);
                    }
                    let body_txt = resp.text().await.unwrap_or_default();
                    if status.as_u16() == 401 {
                        return Err(WandbError::Auth);
                    }
                    if status.as_u16() == 404 {
                        return Err(WandbError::NotFound(body_txt));
                    }
                    if (500..600).contains(&status.as_u16()) {
                        last_err = Some(WandbError::Server {
                            status: status.as_u16(),
                            body: body_txt,
                        });
                        // fall through to retry
                    } else if (400..500).contains(&status.as_u16()) {
                        return Err(WandbError::BadRequest {
                            status: status.as_u16(),
                            body: body_txt,
                        });
                    } else {
                        return Err(WandbError::Unexpected {
                            status: status.as_u16(),
                            body: body_txt,
                        });
                    }
                }
                Err(e) => {
                    if e.is_timeout() || e.is_connect() {
                        last_err = Some(WandbError::Network(e));
                    } else {
                        return Err(WandbError::Network(e));
                    }
                }
            }
            let backoff_ms = RETRY_BASE_MS * (1 << attempt);
            warn!(
                attempt = attempt + 1,
                backoff_ms, "wandb request retry pending"
            );
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
        Err(last_err.unwrap_or_else(|| WandbError::Server {
            status: 0,
            body: "exhausted retries with no error captured".into(),
        }))
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value, WandbError> {
        let url = self.graphql_url();
        let body = json!({ "query": query, "variables": variables });
        let resp = self.post_json(&url, &body).await?;

        // GraphQL uses a 200-with-errors-array convention even for failures.
        // Surface those as `GraphQl` so callers can match on them without
        // sniffing HTTP status codes.
        if let Some(errors) = resp.get("errors").and_then(|v| v.as_array()) {
            if !errors.is_empty() {
                let summary = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(WandbError::GraphQl(summary));
            }
        }
        Ok(resp)
    }

    /// Resolve the api-key owner's default entity (= username). Used when
    /// the user hasn't pinned an entity in config — most personal WandB
    /// accounts have exactly one entity (their username).
    pub async fn viewer_entity(&self) -> Result<String, WandbError> {
        let q = "query Viewer { viewer { entity } }";
        let resp = self.graphql(q, json!({})).await?;
        let entity = resp
            .pointer("/data/viewer/entity")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WandbError::Parse("missing data.viewer.entity".into()))?
            .to_string();
        Ok(entity)
    }

    /// Idempotent run create-or-update. WandB lets the client choose the
    /// run name (we pass `xrun-{run_id}`), so a re-run with the same name
    /// reuses the existing bucket — important for poll-daemon restarts.
    pub async fn upsert_run(
        &self,
        entity: &str,
        project: &str,
        run_name: &str,
        display_name: Option<&str>,
        config: Option<&Value>,
        tags: &[String],
    ) -> Result<WandbRunInfo, WandbError> {
        // The mutation accepts a JSONString for `config` — WandB's schema
        // chose to pass it serialized rather than as a structured object,
        // so we serde_json-encode the user's hyperparams once here.
        let config_str = match config {
            Some(v) => {
                serde_json::to_string(v).map_err(|e| WandbError::Parse(format!("config: {e}")))?
            }
            None => "{}".to_string(),
        };
        let q = "mutation UpsertBucket(\
                $name: String, \
                $entity: String, \
                $project: String, \
                $displayName: String, \
                $config: JSONString, \
                $tags: [String!]) {\
            upsertBucket(input: {\
                name: $name, \
                entityName: $entity, \
                modelName: $project, \
                displayName: $displayName, \
                config: $config, \
                tags: $tags}) {\
                bucket { id name displayName project { name entity { name } } }\
            }\
        }";
        let vars = json!({
            "name": run_name,
            "entity": entity,
            "project": project,
            "displayName": display_name,
            "config": config_str,
            "tags": tags,
        });
        let resp = self.graphql(q, vars).await?;
        let bucket = resp
            .pointer("/data/upsertBucket/bucket")
            .ok_or_else(|| WandbError::Parse("missing data.upsertBucket.bucket".into()))?;
        let id = bucket
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WandbError::Parse("bucket.id missing".into()))?
            .to_string();
        let name = bucket
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(run_name)
            .to_string();
        let project_name = bucket
            .pointer("/project/name")
            .and_then(|v| v.as_str())
            .unwrap_or(project)
            .to_string();
        let entity_name = bucket
            .pointer("/project/entity/name")
            .and_then(|v| v.as_str())
            .unwrap_or(entity)
            .to_string();
        Ok(WandbRunInfo {
            id,
            name,
            project: project_name,
            entity: entity_name,
        })
    }

    /// Append `lines` to the run's `wandb-history.jsonl`. `offset` is the
    /// number of lines already in the file before this batch — WandB uses
    /// it to deduplicate when a network retry double-sends. Pass the
    /// running total as `offset` and the count of lines added grows it.
    pub async fn append_history(
        &self,
        entity: &str,
        project: &str,
        run_name: &str,
        offset: u64,
        lines: &[HistoryLine],
    ) -> Result<(), WandbError> {
        if lines.is_empty() {
            return Ok(());
        }
        let url = self.file_stream_url(entity, project, run_name);
        // Each line in `content` is a JSON-encoded HistoryLine — file_stream
        // takes opaque strings and writes them verbatim into the underlying
        // jsonl file. Encoding once here avoids surfacing serialization
        // failure as a partial write.
        let mut content: Vec<String> = Vec::with_capacity(lines.len());
        for ln in lines {
            content.push(
                serde_json::to_string(ln)
                    .map_err(|e| WandbError::Parse(format!("history line: {e}")))?,
            );
        }
        let body = json!({
            "files": {
                "wandb-history.jsonl": {
                    "offset": offset,
                    "content": content,
                }
            }
        });
        self.post_json(&url, &body).await?;
        Ok(())
    }

    /// Mark the run terminal. WandB closes the file_stream and surfaces the
    /// exit code in the run header (Finished / Crashed / Killed).
    pub async fn finalize_run(
        &self,
        entity: &str,
        project: &str,
        run_name: &str,
        exit: ExitCode,
    ) -> Result<(), WandbError> {
        let url = self.file_stream_url(entity, project, run_name);
        let body = json!({ "complete": true, "exitcode": exit.as_i32() });
        self.post_json(&url, &body).await?;
        Ok(())
    }
}

/// Map WandB's GraphQL+REST error mosaic to a single boolean — used by the
/// retry loop above. Public so callers (sink layer) can reuse the same
/// is-this-worth-retrying judgment.
pub fn is_transient(err: &WandbError) -> bool {
    match err {
        WandbError::Network(e) => e.is_timeout() || e.is_connect(),
        WandbError::Server { status, .. } => (500..600).contains(status),
        _ => false,
    }
}

#[allow(dead_code)]
fn _retry_status(_status: StatusCode) -> bool {
    // Shim kept to surface intent; `is_transient` above is the actual
    // decision point. Keeping this commented helps any future audit see
    // that 429 is *not* yet treated as retryable — wandb's rate-limit
    // bucket is per-entity and short-circuiting the retry loop is the
    // safer default.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stream_url_assembles() {
        let c = WandbClient::new("https://api.wandb.ai/", "k");
        assert_eq!(
            c.file_stream_url("ent", "proj", "xrun-01H"),
            "https://api.wandb.ai/files/ent/proj/xrun-01H/file_stream"
        );
    }

    #[test]
    fn graphql_url_strips_trailing_slash() {
        let c = WandbClient::new("https://api.wandb.ai/", "k");
        assert_eq!(c.graphql_url(), "https://api.wandb.ai/graphql");
    }
}
