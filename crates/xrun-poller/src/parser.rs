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

/// Strict Python-identifier check: `[a-zA-Z_][a-zA-Z0-9_]*`. Rejects keys
/// like `numpy>` (from `numpy>=2.0`), `w[0]`, `tobler 0.13.0`, etc. that
/// would otherwise sneak through the `=`/`:` splitters and surface as
/// phantom metrics in `xrun metrics`.
fn is_metric_identifier(k: &str) -> bool {
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn try_parse_kv(s: &str, ts: DateTime<Utc>) -> Vec<Metric> {
    // Require an explicit epoch/step anchor before mining `key=value` pairs.
    // Without this, hyperparameter prints (`dropout=0.2`, `in_ch=2`) and pip
    // dep specs (`numpy>=2.0` inside `tobler 0.13.0 requires numpy>=2.0`)
    // get scraped as fake metrics. `key: value` is exempt — both because
    // it's a stronger training-output signal (PyTorch/Keras/HF formatting)
    // and because the existing tests exercise it sans-epoch.
    let step_anchor = extract_step(s);
    let step = step_anchor.unwrap_or(0);
    let step_keys: &[&str] = &["epoch", "step", "iter", "global_step"];

    let mut metrics = Vec::new();
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].trim_matches(['[', ']', '(', ')', ',', ';', '|']);

        if let Some((k, v)) = tok.split_once('=') {
            // `key=value` in one token — only when an epoch/step anchor is
            // present in the same line.
            if step_anchor.is_some() {
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim().trim_matches(['[', ']', ',', ';']);
                if is_metric_identifier(&k) && !step_keys.contains(&k.as_str()) {
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
        } else if tok.ends_with(':') {
            // `key:` — value is the next whitespace-separated token
            let k = tok.trim_end_matches(':').trim().to_ascii_lowercase();
            if is_metric_identifier(&k) && !step_keys.contains(&k.as_str()) {
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
            if is_metric_identifier(&k) && !step_keys.contains(&k.as_str()) {
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

    // Regression: arborust v9-skipalpha session captured phantom metrics from
    // hyperparameter prints and pip dep specs because `try_parse_kv` accepted
    // any `<word>=<float>`-shaped token. Tighten by (a) strict identifier on
    // the key and (b) requiring an explicit epoch/step anchor for the `=`
    // form. The `:` form is left alone — training frameworks print it,
    // setup-phase logs rarely do.

    #[test]
    fn no_phantom_metrics_from_pip_dep_spec() {
        // `numpy>=2.0` would split at `=` to k="numpy>", v="2.0" — the `>`
        // breaks the identifier check and the line has no epoch anchor.
        let line = b"tobler 0.13.0 requires numpy>=2.0, but you have numpy 1.26.4";
        let ms = parse_stdout_metrics(line, ts());
        assert!(
            ms.is_empty(),
            "pip dep spec must not surface as a metric: {ms:?}"
        );
    }

    #[test]
    fn no_phantom_metrics_from_hyperparam_print() {
        // Plain `key=value` with no epoch token in the line — typical of
        // model-config prints during setup.
        for line in [
            b"dropout=0.2 in_ch=2 sigma=1.5".as_slice(),
            b"running with batch_size=4 lr=0.001".as_slice(),
        ] {
            let ms = parse_stdout_metrics(line, ts());
            assert!(
                ms.is_empty(),
                "no-epoch line must not yield metrics: {:?} from {:?}",
                ms,
                std::str::from_utf8(line).unwrap()
            );
        }
    }

    #[test]
    fn no_phantom_metrics_from_indexed_repr() {
        // `w[0]=2.74` — the `[0]` makes "w[0]" not a valid identifier.
        let line = b"epoch=3 w[0]=2.74 w[-1]=0.89";
        let ms = parse_stdout_metrics(line, ts());
        let keys: Vec<&str> = ms.iter().map(|m| m.key.as_str()).collect();
        assert!(
            !keys.iter().any(|k| k.contains('[')),
            "indexed reprs must not become metric keys: {keys:?}"
        );
    }

    #[test]
    fn equals_form_still_works_with_epoch_anchor() {
        // The legitimate training-output case must keep working — `kv_equals`
        // already covers this but pin it explicitly post-tightening.
        let line = b"epoch=5 train_loss=0.42 val_f1=0.89";
        let ms = parse_stdout_metrics(line, ts());
        let keys: Vec<&str> = ms.iter().map(|m| m.key.as_str()).collect();
        assert!(keys.contains(&"train_loss"));
        assert!(keys.contains(&"val_f1"));
        assert!(ms.iter().all(|m| m.step == 5));
    }
}
