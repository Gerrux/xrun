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

/// `poll_offsets` keys: bytes of events.jsonl / metrics.jsonl already
/// ingested from the live MLflow chunks. The chunks are the same files read
/// front to back, so the post-run copy only adds what lies past them.
pub const LIVE_EVENTS_OFFSET: &str = "telemetry_events";
pub const LIVE_METRICS_OFFSET: &str = "telemetry_metrics";

/// The part of `content` past what the live stream already delivered.
/// A file shorter than the streamed bytes is not the streamed file: nothing
/// is taken from it rather than risk duplicates (events are not deduped).
fn past_live_offset(content: &[u8], live_offset: u64) -> &[u8] {
    usize::try_from(live_offset)
        .ok()
        .and_then(|skip| content.get(skip..))
        .unwrap_or(&[])
}

/// Parse `events.jsonl` and `metrics.jsonl` from a downloaded kernel output
/// directory and ingest them into the store, skipping the bytes the live
/// MLflow stream already ingested and advancing those offsets to the end.
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
        let raw = std::fs::read(&events_path)?;
        let live = store
            .get_poll_offset(run_id, LIVE_EVENTS_OFFSET)
            .unwrap_or(0);
        let content = String::from_utf8_lossy(past_live_offset(&raw, live)).into_owned();
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
        let end = (raw.len() as u64).max(live);
        if let Err(e) = store.update_poll_offset(run_id, LIVE_EVENTS_OFFSET, end) {
            tracing::warn!("kaggle ingest: could not record events offset: {e}");
        }
    }

    if metrics_path.exists() {
        let raw = std::fs::read(&metrics_path)?;
        let live = store
            .get_poll_offset(run_id, LIVE_METRICS_OFFSET)
            .unwrap_or(0);
        let content = String::from_utf8_lossy(past_live_offset(&raw, live)).into_owned();
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
        let end = (raw.len() as u64).max(live);
        if let Err(e) = store.update_poll_offset(run_id, LIVE_METRICS_OFFSET, end) {
            tracing::warn!("kaggle ingest: could not record metrics offset: {e}");
        }
    }

    Ok((event_count, metric_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EV1: &str = r#"{"ts":"2026-10-15T07:40:00.000Z","stage":"epoch","status":"ok"}"#;
    const EV2: &str = r#"{"ts":"2026-10-15T07:57:00.000Z","stage":"done","status":"ok"}"#;
    const M1: &str = r#"{"step":1,"key":"val_f1","value":0.5,"ts":"2026-10-15T07:40:00.000Z"}"#;

    fn fixture() -> (tempfile::TempDir, Store, RunId) {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&tmp.path().join("runs.db")).unwrap();
        let run_id = store
            .create_run("t", "hash", "m.yaml", "kaggle", &[])
            .unwrap();
        std::fs::create_dir_all(tmp.path().join("out/run")).unwrap();
        (tmp, store, run_id)
    }

    fn write(tmp: &tempfile::TempDir, name: &str, lines: &[&str]) -> u64 {
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(tmp.path().join("out/run").join(name), &body).unwrap();
        body.len() as u64
    }

    #[test]
    fn post_run_ingest_skips_what_the_live_stream_delivered() {
        let (tmp, mut store, run_id) = fixture();
        let first = format!("{EV1}\n").len() as u64;
        let total = write(&tmp, "events.jsonl", &[EV1, EV2]);
        // The live stream already ingested the first line.
        store
            .update_poll_offset(&run_id, LIVE_EVENTS_OFFSET, first)
            .unwrap();

        let (ev, _) = ingest_post_run(&tmp.path().join("out"), &mut store, &run_id).unwrap();

        assert_eq!(ev, 1, "only the line past the live offset is new");
        assert_eq!(
            store.get_poll_offset(&run_id, LIVE_EVENTS_OFFSET).unwrap(),
            total
        );
        // A second pull of the same output adds nothing.
        let (ev, _) = ingest_post_run(&tmp.path().join("out"), &mut store, &run_id).unwrap();
        assert_eq!(ev, 0);
    }

    #[test]
    fn post_run_ingest_takes_everything_without_live_stream() {
        let (tmp, mut store, run_id) = fixture();
        write(&tmp, "events.jsonl", &[EV1, EV2]);
        write(&tmp, "metrics.jsonl", &[M1]);
        let counts = ingest_post_run(&tmp.path().join("out"), &mut store, &run_id).unwrap();
        assert_eq!(counts, (2, 1));
    }

    #[test]
    fn file_shorter_than_the_live_stream_adds_nothing() {
        let (tmp, mut store, run_id) = fixture();
        write(&tmp, "events.jsonl", &[EV1]);
        store
            .update_poll_offset(&run_id, LIVE_EVENTS_OFFSET, 10_000)
            .unwrap();
        let (ev, _) = ingest_post_run(&tmp.path().join("out"), &mut store, &run_id).unwrap();
        assert_eq!(ev, 0);
        assert_eq!(
            store.get_poll_offset(&run_id, LIVE_EVENTS_OFFSET).unwrap(),
            10_000,
            "the live offset is not moved backwards"
        );
    }
}
