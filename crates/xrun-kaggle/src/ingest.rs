#![deny(unsafe_code)]

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use xrun_core::store::{NewEvent, NewMetric, RunId, Store};

use crate::error::KaggleError;

#[derive(Debug, Deserialize)]
struct RawEvent {
    ts: DateTime<Utc>,
    stage: String,
    status: String,
    msg: Option<String>,
    #[serde(flatten)]
    extra: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RawMetric {
    step: i64,
    key: String,
    value: f64,
    ts: DateTime<Utc>,
}

/// Parse `events.jsonl` and `metrics.jsonl` from a downloaded kernel output
/// directory and ingest them into the store.
pub fn ingest_post_run(
    output_dir: &Path,
    store: &mut Store,
    run_id: &RunId,
) -> Result<(usize, usize), KaggleError> {
    let run_subdir = output_dir.join("run");

    let events_path = run_subdir.join("events.jsonl");
    let metrics_path = run_subdir.join("metrics.jsonl");

    let mut event_count = 0;
    let mut metric_count = 0;

    if events_path.exists() {
        let content = std::fs::read_to_string(&events_path)?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<RawEvent>(line) {
                Ok(ev) => {
                    let payload_json = ev.extra.as_ref().and_then(|v| {
                        if v.is_object() && v.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                            Some(v.to_string())
                        } else {
                            None
                        }
                    });
                    let _ = store.append_event(
                        run_id,
                        NewEvent {
                            ts: ev.ts,
                            stage: ev.stage,
                            status: ev.status,
                            msg: ev.msg,
                            payload_json,
                        },
                    );
                    event_count += 1;
                }
                Err(e) => {
                    tracing::warn!("failed to parse event line: {e} — line: {line}");
                }
            }
        }
    }

    if metrics_path.exists() {
        let content = std::fs::read_to_string(&metrics_path)?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<RawMetric>(line) {
                Ok(m) => {
                    let _ = store.append_metric(
                        run_id,
                        NewMetric {
                            step: m.step,
                            key: m.key,
                            value: m.value,
                            ts: m.ts,
                        },
                    );
                    metric_count += 1;
                }
                Err(e) => {
                    tracing::warn!("failed to parse metric line: {e} — line: {line}");
                }
            }
        }
    }

    Ok((event_count, metric_count))
}
