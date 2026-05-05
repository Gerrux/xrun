#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};

/// One row in the streamed `wandb-history.jsonl` payload. WandB's UI joins
/// these on `_step` to plot multi-key runs, so callers should set the same
/// `_step` for metrics that share an epoch / iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryLine {
    /// Iteration / step counter. Required — without it WandB plots flatten.
    #[serde(rename = "_step")]
    pub step: i64,
    /// Seconds since run start. Optional — WandB infers from server-side
    /// receive time when missing, but providing it avoids drift between
    /// poller-write time and server-receive time.
    #[serde(rename = "_runtime", skip_serializing_if = "Option::is_none")]
    pub runtime_secs: Option<f64>,
    /// Unix epoch seconds (float — WandB tolerates ms precision via decimals).
    #[serde(rename = "_timestamp", skip_serializing_if = "Option::is_none")]
    pub timestamp_secs: Option<f64>,
    /// Per-key metric values. Keys here become column names in the WandB
    /// run table; flat scalars only.
    #[serde(flatten)]
    pub values: serde_json::Map<String, serde_json::Value>,
}

/// What `upsertBucket` returns. Shape mirrors the GraphQL response we
/// actually need — full schema is bigger.
#[derive(Debug, Clone)]
pub struct WandbRunInfo {
    /// WandB internal ID — the GUID-like string used in API URLs.
    pub id: String,
    /// Human-readable run name. We pass `xrun-{run_id}` so it round-trips
    /// to the local store.
    pub name: String,
    pub project: String,
    pub entity: String,
}

impl WandbRunInfo {
    pub fn web_url(&self, web_base: &str) -> String {
        format!(
            "{}/{}/{}/runs/{}",
            web_base.trim_end_matches('/'),
            self.entity,
            self.project,
            self.name,
        )
    }
}

/// Exit status for `file_stream` `complete: true` payloads. WandB uses 0 for
/// success, anything non-zero is rendered as "Crashed" / "Failed" depending
/// on which signal it maps to. We pick the closest fit for xrun's three
/// terminal statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success,
    Failure,
    Killed,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        match self {
            ExitCode::Success => 0,
            ExitCode::Failure => 1,
            // SIGTERM (128 + 15) so WandB UI shows the run as killed/cancelled
            // instead of generic "failed".
            ExitCode::Killed => 143,
        }
    }
}
