#![deny(unsafe_code)]

use std::io::BufReader;

use xrun_core::{Event, JsonlReader, Metric, MetricsJsonlReader};

/// Parse JSONL event bytes. Malformed lines are logged and skipped.
pub fn parse_events(bytes: &[u8]) -> Vec<Event> {
    let mut out = Vec::new();
    for item in JsonlReader::new(BufReader::new(bytes)) {
        match item {
            Ok(ev) => out.push(ev),
            Err(e) => tracing::warn!("skipping malformed event line: {e}"),
        }
    }
    out
}

/// Parse JSONL metric bytes. Malformed lines are logged and skipped.
pub fn parse_metrics(bytes: &[u8]) -> Vec<Metric> {
    let mut out = Vec::new();
    for item in MetricsJsonlReader::new(BufReader::new(bytes)) {
        match item {
            Ok(m) => out.push(m),
            Err(e) => tracing::warn!("skipping malformed metric line: {e}"),
        }
    }
    out
}
