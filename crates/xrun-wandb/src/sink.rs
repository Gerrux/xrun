#![deny(unsafe_code)]

//! `MetricSink` impl backed by `WandbClient`.
//!
//! Maps `xrun-core` types to WandB's run + history-stream model:
//!
//! - `OpenRunCtx.experiment` → WandB project name (manifest `name` field).
//! - `OpenRunCtx.run_id` → WandB run name (`xrun-{ulid}`), so a run-detail
//!   URL is predictable from xrun's side without round-tripping the bucket id.
//! - `OpenRunCtx.config` → WandB run `config` (mirrors hyperparams to the
//!   run's overview tab).
//! - `OpenRunCtx.tags` → WandB tags (plus auto-added `vendor:<X>`,
//!   `xrun_run_id:<X>`).
//! - `MetricPoint{key,value,step,ts}` → one `HistoryLine` per *step* (we
//!   merge points sharing a step before sending so WandB plots them on the
//!   same x-axis tick).
//!
//! `log_artifact` is intentionally not yet implemented — uploading
//! checkpoints to WandB's artifact store needs the artifacts API which is a
//! separate slice. For now it returns `Disabled` so callers degrade silent.
//!
//! Concurrency: the sink keeps an `AtomicU64` history offset (per run) so
//! `log_metrics_batch` calls can interleave without WandB rejecting the
//! second one as a duplicate-offset write. Concurrency is single-poller in
//! practice but the atomic costs nothing and protects against future
//! parallel pollers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use xrun_core::metric_sink::{
    MetricPoint, MetricSink, MetricSinkError, OpenRunCtx, RemoteRunHandle,
};
use xrun_core::store::RunStatus;

use crate::client::{WandbClient, DEFAULT_WEB_BASE};
use crate::error::WandbError;
use crate::types::{ExitCode, HistoryLine};

fn map_err(e: WandbError) -> MetricSinkError {
    match e {
        WandbError::Auth => MetricSinkError::Auth("WandB rejected api key (401)".into()),
        WandbError::Network(inner) => MetricSinkError::Network(inner.to_string()),
        WandbError::GraphQl(s) => MetricSinkError::Server(format!("WandB GraphQL: {s}")),
        WandbError::BadRequest { status, body } => {
            MetricSinkError::Server(format!("WandB bad request (HTTP {status}): {body}"))
        }
        WandbError::NotFound(b) => MetricSinkError::Server(format!("WandB 404: {b}")),
        WandbError::Server { status, body } => {
            MetricSinkError::Server(format!("WandB server (HTTP {status}): {body}"))
        }
        WandbError::Unexpected { status, body } => {
            MetricSinkError::Server(format!("WandB unexpected (HTTP {status}): {body}"))
        }
        WandbError::Parse(s) => MetricSinkError::Server(format!("WandB parse: {s}")),
        WandbError::Config(s) => MetricSinkError::Config(s),
    }
}

/// Per-handle scratch state. Kept outside `RemoteRunHandle` (which is owned
/// by the trait) and looked up by `remote_run_id`.
struct RunState {
    entity: String,
    project: String,
    run_name: String,
    /// Lines already streamed to wandb-history.jsonl. Used as `offset` on
    /// every append so retries don't double-write.
    history_offset: AtomicU64,
    /// Captured at run-open so each `HistoryLine` can populate `_runtime`.
    started_at: chrono::DateTime<chrono::Utc>,
}

pub struct WandbSink {
    client: WandbClient,
    /// Resolved entity (or `None` until the first `open_run`, in which case
    /// we probe `viewer { entity }`). Cached so subsequent runs skip the
    /// extra round-trip.
    entity: Mutex<Option<String>>,
    /// Public web URL prefix used when building run links. Defaults to
    /// `https://wandb.ai`; override for self-hosted Weave instances.
    web_base: String,
    /// `remote_run_id` → per-run state. Bounded by the number of currently-
    /// active runs the poller is mirroring (typically 1 — the daemon owns
    /// one run at a time).
    runs: Mutex<HashMap<String, RunState>>,
}

impl WandbSink {
    pub fn new(client: WandbClient, default_entity: Option<String>) -> Self {
        Self {
            client,
            entity: Mutex::new(default_entity),
            web_base: DEFAULT_WEB_BASE.to_string(),
            runs: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_web_base(mut self, web_base: impl Into<String>) -> Self {
        self.web_base = web_base.into().trim_end_matches('/').to_string();
        self
    }

    /// Resolve entity: cached value, or probe `viewer { entity }`.
    async fn resolve_entity(&self) -> Result<String, MetricSinkError> {
        if let Some(e) = self.entity.lock().expect("entity mutex poisoned").clone() {
            return Ok(e);
        }
        let e = self.client.viewer_entity().await.map_err(map_err)?;
        *self.entity.lock().expect("entity mutex poisoned") = Some(e.clone());
        Ok(e)
    }

    fn run_name_for(run_id: &str) -> String {
        // Prefix with `xrun-` so the wandb dashboard view filter is one
        // search away, and so manually-created runs never collide.
        format!("xrun-{run_id}")
    }
}

#[async_trait]
impl MetricSink for WandbSink {
    fn name(&self) -> &str {
        "wandb"
    }

    async fn open_run(&self, ctx: OpenRunCtx<'_>) -> Result<RemoteRunHandle, MetricSinkError> {
        let entity = self.resolve_entity().await?;
        let project = ctx.experiment.to_string();
        let run_name = Self::run_name_for(ctx.run_id);

        // Tags: caller's free-form set + auto markers so the wandb UI's
        // tag filter matches the same conventions as MLflow.
        let mut tags: Vec<String> = ctx.tags.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        tags.push(format!("vendor:{}", ctx.vendor));
        if let Some(inst) = ctx.instance_id {
            tags.push(format!("instance:{inst}"));
        }
        tags.push(format!("xrun_run_id:{}", ctx.run_id));

        // Hyperparams → wandb config object. Pass through verbatim — the
        // serde_json::Value already preserves int/float/string types so
        // wandb's config viewer renders them correctly.
        let config_obj = if ctx.config.is_empty() {
            None
        } else {
            let map: serde_json::Map<String, serde_json::Value> = ctx
                .config
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Some(serde_json::Value::Object(map))
        };

        let info = self
            .client
            .upsert_run(
                &entity,
                &project,
                &run_name,
                ctx.run_name,
                config_obj.as_ref(),
                &tags,
            )
            .await
            .map_err(map_err)?;

        let url = info.web_url(&self.web_base);

        // Stash per-handle scratch state. Keyed by `remote_run_id` (the
        // wandb bucket id) so subsequent calls can recover entity/project
        // without round-tripping wandb again.
        let state = RunState {
            entity: info.entity.clone(),
            project: info.project.clone(),
            run_name: info.name.clone(),
            history_offset: AtomicU64::new(0),
            started_at: Utc::now(),
        };
        self.runs
            .lock()
            .expect("runs mutex poisoned")
            .insert(info.id.clone(), state);

        Ok(RemoteRunHandle {
            sink_name: "wandb".into(),
            remote_run_id: info.id,
            remote_url: Some(url),
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
        // Read state (entity/project/run_name + offset + start time) once
        // up front. Drop the lock before doing the HTTP call so
        // log_metrics_batch and finalize don't serialize on each other.
        let (entity, project, run_name, offset_anchor, started_at) = {
            let runs = self.runs.lock().expect("runs mutex poisoned");
            let st = runs.get(&handle.remote_run_id).ok_or_else(|| {
                MetricSinkError::Other(format!(
                    "wandb: unknown handle {} — open_run was never called or sink dropped state",
                    handle.remote_run_id
                ))
            })?;
            (
                st.entity.clone(),
                st.project.clone(),
                st.run_name.clone(),
                st.history_offset.load(Ordering::SeqCst),
                st.started_at,
            )
        };

        // Group `MetricPoint`s by step so multi-key updates within one
        // training iteration land on the same wandb x-axis tick. Without
        // this grouping wandb plots loss + acc on different points even
        // when they came from the same `xrun_hook.metrics({...}, step=N)`
        // call.
        let mut by_step: std::collections::BTreeMap<i64, HistoryLine> =
            std::collections::BTreeMap::new();
        for p in batch {
            let entry = by_step.entry(p.step).or_insert_with(|| {
                let runtime_secs = (p.timestamp_ms as f64 / 1000.0) - started_at.timestamp() as f64;
                HistoryLine {
                    step: p.step,
                    runtime_secs: Some(runtime_secs.max(0.0)),
                    timestamp_secs: Some(p.timestamp_ms as f64 / 1000.0),
                    values: serde_json::Map::new(),
                }
            });
            entry
                .values
                .insert(p.key.clone(), serde_json::Value::from(p.value));
        }
        let lines: Vec<HistoryLine> = by_step.into_values().collect();
        let line_count = lines.len() as u64;

        self.client
            .append_history(&entity, &project, &run_name, offset_anchor, &lines)
            .await
            .map_err(map_err)?;

        // Bump the offset only after the server acked the write — a failure
        // mid-call leaves the offset at the previous value so a retry will
        // overwrite the same range (wandb dedupes on offset+content match).
        if let Some(st) = self
            .runs
            .lock()
            .expect("runs mutex poisoned")
            .get(&handle.remote_run_id)
        {
            st.history_offset.fetch_add(line_count, Ordering::SeqCst);
        }
        Ok(())
    }

    async fn log_artifact(
        &self,
        _handle: &RemoteRunHandle,
        _path: &Path,
        _name: &str,
    ) -> Result<(), MetricSinkError> {
        // Artifact upload uses WandB's separate manifests + S3-presigned
        // PUT API which is a different surface from file_stream. Slice 4+
        // can wire it; until then degrade silent so checkpoint pulls don't
        // surface as wandb errors.
        Err(MetricSinkError::Disabled(
            "wandb log_artifact not yet implemented; use --pull for local copy".into(),
        ))
    }

    async fn finalize(
        &self,
        handle: &RemoteRunHandle,
        status: RunStatus,
    ) -> Result<(), MetricSinkError> {
        let exit = match status {
            RunStatus::Done => ExitCode::Success,
            RunStatus::Failed => ExitCode::Failure,
            RunStatus::Cancelled => ExitCode::Killed,
            // Defensive: unfinished states (Provisioning/Running/Queued)
            // can land here on a daemon Ctrl-C; treat as Killed so wandb's
            // run-list filter shows the run as terminated rather than
            // perpetually-running.
            _ => ExitCode::Killed,
        };
        let (entity, project, run_name) = {
            let runs = self.runs.lock().expect("runs mutex poisoned");
            let st = runs.get(&handle.remote_run_id).ok_or_else(|| {
                MetricSinkError::Other(format!("wandb: unknown handle {}", handle.remote_run_id))
            })?;
            (st.entity.clone(), st.project.clone(), st.run_name.clone())
        };
        self.client
            .finalize_run(&entity, &project, &run_name, exit)
            .await
            .map_err(map_err)?;

        // Drop the per-run state — caller may keep the handle, but
        // post-finalize calls on wandb error out anyway.
        self.runs
            .lock()
            .expect("runs mutex poisoned")
            .remove(&handle.remote_run_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_name_carries_xrun_prefix() {
        assert_eq!(WandbSink::run_name_for("01H1234"), "xrun-01H1234");
    }

    #[test]
    fn map_err_classifies_auth() {
        let e = map_err(WandbError::Auth);
        assert!(matches!(e, MetricSinkError::Auth(_)));
    }

    #[test]
    fn map_err_classifies_config() {
        let e = map_err(WandbError::Config("missing entity".into()));
        assert!(matches!(e, MetricSinkError::Config(_)));
    }

    #[test]
    fn map_err_classifies_5xx_as_server() {
        let e = map_err(WandbError::Server {
            status: 502,
            body: "bad gw".into(),
        });
        assert!(matches!(e, MetricSinkError::Server(_)));
    }
}
