#![deny(unsafe_code)]

use std::io::BufReader;

use chrono::{DateTime, Utc};
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

/// Extract numeric metrics from a single stdout line without requiring xrun_hook.
///
/// Recognised patterns (common across PyTorch, Keras, fastai, HuggingFace):
///   1. JSONL: `{"epoch":5,"loss":0.42,"val_f1":0.89}` — all numeric fields;
///      `epoch`, `step`, or `iter` fields are used as the step counter.
///   2. `epoch=5 loss=0.42 val_f1=0.89` — space/comma/semicolon-separated
///      `key=value` pairs; `epoch`/`step`/`iter` become the step counter.
///   3. `[Epoch 5] loss: 0.42, val_acc: 0.95` — bracketed epoch header,
///      then colon-separated `key: value` pairs.
///   4. `Epoch 5/10 — loss: 0.42` — inline epoch fraction at line start.
///
/// Returns an empty Vec when no parseable numeric metrics are found. Non-numeric
/// values and keys containing whitespace are skipped silently.
pub fn parse_stdout_metrics(line: &[u8], ts: DateTime<Utc>) -> Vec<Metric> {
    let s = match std::str::from_utf8(line) {
        Ok(s) => s.trim(),
        Err(_) => return Vec::new(),
    };
    if s.is_empty() || s.starts_with('#') || s.starts_with("//") {
        return Vec::new();
    }

    // Pattern 1: JSONL
    if s.starts_with('{') {
        if let Some(metrics) = try_parse_jsonl(s, ts) {
            if !metrics.is_empty() {
                return metrics;
            }
        }
        return Vec::new(); // Valid JSON but no numeric fields — skip KV fallback
    }

    // Patterns 2/3/4: extract from key=value / key: value pairs
    try_parse_kv(s, ts)
}

fn try_parse_jsonl(s: &str, ts: DateTime<Utc>) -> Option<Vec<Metric>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let obj = v.as_object()?;

    // Step counter: prefer `epoch`, then `step`, then `iter`
    let step = ["epoch", "step", "iter", "global_step"]
        .iter()
        .find_map(|&k| obj.get(k)?.as_f64())
        .map(|f| f as i64)
        .unwrap_or(0);

    let step_keys: &[&str] = &["epoch", "step", "iter", "global_step"];
    let mut metrics = Vec::new();
    for (key, val) in obj {
        if step_keys.contains(&key.as_str()) {
            continue;
        }
        if let Some(f) = val.as_f64() {
            if f.is_finite() {
                metrics.push(Metric {
                    ts,
                    step,
                    key: key.clone(),
                    value: f,
                });
            }
        }
    }
    Some(metrics)
}

/// Extract the epoch/step integer from patterns like `[Epoch 5]`, `Epoch 5/10`,
/// or `epoch=5` anywhere in the line.
fn extract_step(s: &str) -> Option<i64> {
    // `[Epoch 5]` or `[epoch 5/10]` — bracketed header
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let inside = rest[..end].to_ascii_lowercase();
            let stripped = inside
                .strip_prefix("epoch ")
                .or_else(|| inside.strip_prefix("step "));
            if let Some(stripped) = stripped {
                let num = stripped.split('/').next().unwrap_or(stripped);
                if let Ok(v) = num.trim().parse::<i64>() {
                    return Some(v);
                }
            }
        }
    }

    // `Epoch 5/10` at the start of the line (case-insensitive)
    let lower = s.to_ascii_lowercase();
    for prefix in &["epoch ", "step "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let num = rest.split_whitespace().next().unwrap_or("");
            let num = num.trim_end_matches(':').split('/').next().unwrap_or(num);
            if let Ok(v) = num.parse::<i64>() {
                return Some(v);
            }
        }
    }

    // `epoch=5` or `step=5` anywhere in whitespace-split tokens
    for token in s.split_whitespace() {
        let tl = token.to_ascii_lowercase();
        for prefix in &["epoch=", "step=", "iter=", "global_step="] {
            if let Some(val) = tl.strip_prefix(prefix) {
                let val = val.split('/').next().unwrap_or(val).trim_matches(',');
                if let Ok(v) = val.parse::<i64>() {
                    return Some(v);
                }
            }
        }
    }

    None
}

fn try_parse_kv(s: &str, ts: DateTime<Utc>) -> Vec<Metric> {
    let step = extract_step(s).unwrap_or(0);
    let step_keys: &[&str] = &["epoch", "step", "iter", "global_step"];

    let mut metrics = Vec::new();
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].trim_matches(['[', ']', '(', ')', ',', ';', '|']);

        if let Some((k, v)) = tok.split_once('=') {
            // `key=value` in one token
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().trim_matches(['[', ']', ',', ';']);
            if !k.is_empty() && !k.contains(' ') && !step_keys.contains(&k.as_str()) {
                if let Ok(f) = v.parse::<f64>() {
                    if f.is_finite() {
                        metrics.push(Metric {
                            ts,
                            step,
                            key: k,
                            value: f,
                        });
                    }
                }
            }
        } else if tok.ends_with(':') {
            // `key:` — value is the next whitespace-separated token
            let k = tok.trim_end_matches(':').trim().to_ascii_lowercase();
            if !k.is_empty() && !k.contains(' ') && !step_keys.contains(&k.as_str()) {
                if let Some(next) = tokens.get(i + 1) {
                    let v = next.trim_matches(['[', ']', ',', ';']);
                    if let Ok(f) = v.parse::<f64>() {
                        if f.is_finite() {
                            metrics.push(Metric {
                                ts,
                                step,
                                key: k,
                                value: f,
                            });
                            i += 1; // consume the value token
                        }
                    }
                }
            }
        } else if let Some((k, v)) = tok.split_once(':') {
            // `key:value` without space (e.g. `loss:0.42`)
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().trim_matches(['[', ']', ',', ';']);
            if !k.is_empty() && !k.contains(' ') && !step_keys.contains(&k.as_str()) {
                if let Ok(f) = v.parse::<f64>() {
                    if f.is_finite() {
                        metrics.push(Metric {
                            ts,
                            step,
                            key: k,
                            value: f,
                        });
                    }
                }
            }
        }
        i += 1;
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn ts() -> DateTime<Utc> {
        Utc::now()
    }

    fn keys(ms: &[Metric]) -> Vec<&str> {
        ms.iter().map(|m| m.key.as_str()).collect()
    }

    #[test]
    fn jsonl_epoch_and_metrics() {
        let line = br#"{"epoch":5,"loss":0.42,"val_f1":0.89}"#;
        let ms = parse_stdout_metrics(line, ts());
        assert_eq!(ms.len(), 2);
        assert!(ms.iter().all(|m| m.step == 5));
        assert!(keys(&ms).contains(&"loss"));
        assert!(keys(&ms).contains(&"val_f1"));
    }

    #[test]
    fn kv_equals() {
        let line = b"epoch=3 loss=0.123 acc=0.97";
        let ms = parse_stdout_metrics(line, ts());
        assert_eq!(ms.len(), 2);
        assert!(ms.iter().all(|m| m.step == 3));
    }

    #[test]
    fn bracketed_epoch() {
        let line = b"[Epoch 7] loss: 0.21, val_acc: 0.93";
        let ms = parse_stdout_metrics(line, ts());
        assert_eq!(ms.len(), 2);
        assert!(ms.iter().all(|m| m.step == 7));
    }

    #[test]
    fn empty_and_comment_lines_skipped() {
        assert!(parse_stdout_metrics(b"", ts()).is_empty());
        assert!(parse_stdout_metrics(b"# comment", ts()).is_empty());
        assert!(parse_stdout_metrics(b"No metrics here at all", ts()).is_empty());
    }
}
