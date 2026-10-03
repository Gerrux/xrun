#![deny(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_yaml::Value as Yaml;
use xrun_core::{Run, RunId, Store, StoredMetric};

use crate::cli::DiffArgs;
use crate::commands::common::{open_store, resolve_run};

pub fn run(args: &DiffArgs, db_path: &Path, runs_dir: &Path) -> Result<()> {
    let store = open_store(db_path)?;
    let run_a = resolve_run(&store, &args.a)?;
    let run_b = resolve_run(&store, &args.b)?;
    let (id_a, id_b) = (run_a.id.clone(), run_b.id.clone());

    // Validate before touching the filesystem so a typo fails fast.
    let overrides = parse_direction_overrides(&args.direction)?;

    // The manifest diff needs both manifests (hard error if missing); the
    // metrics section only borrows early_stop from them, so a missing file
    // there just drops to the name heuristic.
    let (yaml_a, yaml_b) = if args.manifest_only || !args.metrics_only {
        (
            Some(load_manifest_yaml(runs_dir, &run_a)?),
            Some(load_manifest_yaml(runs_dir, &run_b)?),
        )
    } else {
        (
            load_manifest_yaml(runs_dir, &run_a).ok(),
            load_manifest_yaml(runs_dir, &run_b).ok(),
        )
    };

    let manifest_diff = match (&yaml_a, &yaml_b) {
        (Some(a), Some(b)) if !args.metrics_only => compute_manifest_diff(a, b),
        _ => Vec::new(),
    };

    let metrics_diff = if args.manifest_only {
        Vec::new()
    } else {
        let key_filter: Option<Vec<String>> = args
            .keys
            .as_deref()
            .map(|s| s.split(',').map(str::trim).map(str::to_string).collect());
        let manifests = [yaml_a.as_ref(), yaml_b.as_ref()];
        compute_metrics_diff(
            &store,
            &id_a,
            &id_b,
            key_filter.as_deref(),
            &overrides,
            &manifests,
        )?
    };

    if args.json {
        print_json(&run_a, &run_b, &manifest_diff, &metrics_diff);
    } else {
        print_text(&run_a, &run_b, &manifest_diff, &metrics_diff, args);
    }

    Ok(())
}

fn load_manifest_yaml(runs_dir: &Path, run: &Run) -> Result<Yaml> {
    let path = runs_dir.join(run.id.to_string()).join("manifest.yaml");
    let content = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read manifest for run {}: {}",
            run.id,
            path.display()
        )
    })?;
    serde_yaml::from_str(&content)
        .with_context(|| format!("failed to parse manifest at {}", path.display()))
}

// ---------------------------------------------------------------------------
// Manifest diff
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ManifestDiffEntry {
    pub path: String,
    pub a: Option<serde_json::Value>,
    pub b: Option<serde_json::Value>,
}

/// Recursively compare two YAML values; emit one entry per leaf difference.
/// Maps are descended key-by-key; sequences are descended index-by-index.
/// When shapes mismatch (map vs seq vs scalar), emit a single entry at the
/// current path with both whole subtrees.
pub fn compute_manifest_diff(a: &Yaml, b: &Yaml) -> Vec<ManifestDiffEntry> {
    let mut out = Vec::new();
    walk_diff("", a, b, &mut out);
    out
}

fn walk_diff(path: &str, a: &Yaml, b: &Yaml, out: &mut Vec<ManifestDiffEntry>) {
    match (a, b) {
        (Yaml::Mapping(ma), Yaml::Mapping(mb)) => {
            // Stable order: keys from a first, then keys-only-in-b in their original order.
            let mut seen: Vec<String> = Vec::new();
            for (k, va) in ma {
                let key = yaml_key_to_string(k);
                let child_path = join_path(path, &key);
                seen.push(key.clone());
                match mb.get(k) {
                    Some(vb) => walk_diff(&child_path, va, vb, out),
                    None => out.push(ManifestDiffEntry {
                        path: child_path,
                        a: Some(yaml_to_json(va)),
                        b: None,
                    }),
                }
            }
            for (k, vb) in mb {
                let key = yaml_key_to_string(k);
                if seen.contains(&key) {
                    continue;
                }
                out.push(ManifestDiffEntry {
                    path: join_path(path, &key),
                    a: None,
                    b: Some(yaml_to_json(vb)),
                });
            }
        }
        (Yaml::Sequence(sa), Yaml::Sequence(sb)) => {
            let n = sa.len().max(sb.len());
            for i in 0..n {
                let child_path = format!("{path}[{i}]");
                match (sa.get(i), sb.get(i)) {
                    (Some(x), Some(y)) => walk_diff(&child_path, x, y, out),
                    (Some(x), None) => out.push(ManifestDiffEntry {
                        path: child_path,
                        a: Some(yaml_to_json(x)),
                        b: None,
                    }),
                    (None, Some(y)) => out.push(ManifestDiffEntry {
                        path: child_path,
                        a: None,
                        b: Some(yaml_to_json(y)),
                    }),
                    (None, None) => {}
                }
            }
        }
        _ => {
            if a != b {
                out.push(ManifestDiffEntry {
                    path: if path.is_empty() {
                        "(root)".to_string()
                    } else {
                        path.to_string()
                    },
                    a: Some(yaml_to_json(a)),
                    b: Some(yaml_to_json(b)),
                });
            }
        }
    }
}

fn join_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

fn yaml_key_to_string(k: &Yaml) -> String {
    match k {
        Yaml::String(s) => s.clone(),
        Yaml::Bool(b) => b.to_string(),
        Yaml::Number(n) => n.to_string(),
        Yaml::Null => "null".to_string(),
        // Complex keys (rare) — fall back to YAML repr.
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn yaml_to_json(v: &Yaml) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// Metrics diff
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MetricDiffEntry {
    pub key: String,
    /// "min" or "max": `--direction` override, else the manifest's
    /// `policy.early_stop.mode`, else guessed from the key name.
    pub direction: &'static str,
    pub a_last: Option<f64>,
    pub a_best: Option<f64>,
    pub b_last: Option<f64>,
    pub b_best: Option<f64>,
    /// `b_best - a_best`. None if either side has no data.
    pub delta_best: Option<f64>,
}

fn compute_metrics_diff(
    store: &Store,
    id_a: &RunId,
    id_b: &RunId,
    filter: Option<&[String]>,
    overrides: &HashMap<String, &'static str>,
    manifests: &[Option<&Yaml>],
) -> Result<Vec<MetricDiffEntry>> {
    let metrics_a = store
        .list_metrics(id_a, filter)
        .context("failed to list metrics for a")?;
    let metrics_b = store
        .list_metrics(id_b, filter)
        .context("failed to list metrics for b")?;

    // Union of keys in stable (sorted) order.
    let mut keys: Vec<String> = metrics_a
        .iter()
        .chain(metrics_b.iter())
        .map(|m| m.key.clone())
        .collect();
    keys.sort();
    keys.dedup();

    // A typo in KEY would otherwise leave the heuristic in charge, silently.
    for k in unknown_direction_keys(overrides, &keys) {
        eprintln!("warning: --direction '{k}' matches no compared metric of either run; ignored");
    }

    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        let direction = resolve_direction(&key, overrides, manifests);
        let (a_last, a_best) = aggregate(&metrics_a, &key, direction);
        let (b_last, b_best) = aggregate(&metrics_b, &key, direction);
        let delta_best = match (a_best, b_best) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        };
        out.push(MetricDiffEntry {
            key,
            direction,
            a_last,
            a_best,
            b_last,
            b_best,
            delta_best,
        });
    }
    Ok(out)
}

/// `--direction` keys that name no metric in `keys`, sorted.
fn unknown_direction_keys(
    overrides: &HashMap<String, &'static str>,
    keys: &[String],
) -> Vec<String> {
    let mut unknown: Vec<String> = overrides
        .keys()
        .filter(|k| !keys.contains(k))
        .cloned()
        .collect();
    unknown.sort();
    unknown
}

/// Name-only fallback for "which value is best". Keep identical to
/// `metric_direction` in python/xrun_tui/src/xrun_tui/utils.py.
///
/// The key is split on non-alphanumerics and on ASCII camelCase boundaries
/// (`valError` → `val`, `error`; an ALL-CAPS run before a capitalised word
/// splits too: `FIDScore` → `fid`, `score`), and each token is checked (raw
/// substring matching would flag `overall` or `kernel`-style words):
/// - `loss` anywhere in a token (`val_loss`, `mseloss`, `lossy`), except
///   `lossless`;
/// - `err` / `perplexity` as a token prefix (`error`, `errors`, `rel_err`);
/// - `mae mse rmse ppl wer cer fid nll eer bpb bpc` only as a whole token
///   (optionally plural), so `cert`, `fidelity`, `werner` stay
///   higher-is-better.
///
/// Everything else maximises.
pub fn best_direction(key: &str) -> &'static str {
    const EXACT: [&str; 11] = [
        "mae", "mse", "rmse", "ppl", "wer", "cer", "fid", "nll", "eer", "bpb", "bpc",
    ];
    // camelCase boundary becomes a separator: lower/digit then upper, or the
    // last capital of an upper run that precedes a lowercase letter.
    let chars: Vec<char> = key.chars().collect();
    let mut split = String::with_capacity(key.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let p = chars[i - 1];
            let after_lower = p.is_ascii_lowercase() || p.is_ascii_digit();
            let ends_caps_run =
                p.is_ascii_uppercase() && chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if after_lower || ends_caps_run {
                split.push('_');
            }
        }
        split.push(c);
    }
    let k = split.to_lowercase();
    let low = k
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .any(|t| {
            t.replace("lossless", "").contains("loss")
                || t.starts_with("err")
                || t.starts_with("perplexity")
                || EXACT
                    .iter()
                    .any(|w| t == *w || t.strip_suffix('s') == Some(w))
        });
    if low {
        "min"
    } else {
        "max"
    }
}

/// Direction declared by a manifest's `policy.early_stop` when it targets
/// `key`. Mode defaults to max, like `EarlyStopMode`.
fn early_stop_direction(manifest: &Yaml, key: &str) -> Option<&'static str> {
    let es = manifest.get("policy")?.get("early_stop")?;
    if es.get("metric")?.as_str()? != key {
        return None;
    }
    match es.get("mode").and_then(Yaml::as_str) {
        Some("min") => Some("min"),
        _ => Some("max"),
    }
}

/// override → early_stop of run A, then run B → name heuristic.
fn resolve_direction(
    key: &str,
    overrides: &HashMap<String, &'static str>,
    manifests: &[Option<&Yaml>],
) -> &'static str {
    if let Some(d) = overrides.get(key) {
        return d;
    }
    manifests
        .iter()
        .flatten()
        .find_map(|m| early_stop_direction(m, key))
        .unwrap_or_else(|| best_direction(key))
}

/// Parse `--direction KEY=min|max` values into a map.
fn parse_direction_overrides(raw: &[String]) -> Result<HashMap<String, &'static str>> {
    let mut out = HashMap::new();
    for item in raw {
        let item = item.trim();
        let (key, val) = item.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("invalid --direction '{item}': expected KEY=min or KEY=max")
        })?;
        let key = key.trim();
        if key.is_empty() {
            anyhow::bail!("invalid --direction '{item}': empty metric key (expected KEY=min|max)");
        }
        let dir = match val.trim().to_ascii_lowercase().as_str() {
            "min" => "min",
            "max" => "max",
            other => anyhow::bail!(
                "invalid --direction '{item}': value '{other}' is not one of: min, max"
            ),
        };
        // A repeat with the same value is harmless; a contradicting one is
        // a typo, and "last one wins" would hide it.
        if let Some(prev) = out.insert(key.to_string(), dir) {
            if prev != dir {
                anyhow::bail!("conflicting --direction for '{key}': both {prev} and {dir} given");
            }
        }
    }
    Ok(out)
}

fn aggregate(metrics: &[StoredMetric], key: &str, direction: &str) -> (Option<f64>, Option<f64>) {
    let pts: Vec<&StoredMetric> = metrics.iter().filter(|m| m.key == key).collect();
    if pts.is_empty() {
        return (None, None);
    }
    // metrics are pre-sorted by (step, key) so the last for this key is
    // the highest-step one.
    let last = pts.last().map(|m| m.value);
    let best = match direction {
        "min" => pts
            .iter()
            .map(|m| m.value)
            .filter(|v| !v.is_nan())
            .fold(f64::INFINITY, f64::min),
        _ => pts
            .iter()
            .map(|m| m.value)
            .filter(|v| !v.is_nan())
            .fold(f64::NEG_INFINITY, f64::max),
    };
    let best = if best.is_finite() { Some(best) } else { None };
    (last, best)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_json(
    run_a: &Run,
    run_b: &Run,
    manifest_diff: &[ManifestDiffEntry],
    metrics_diff: &[MetricDiffEntry],
) {
    let out = serde_json::json!({
        "a": run_summary(run_a),
        "b": run_summary(run_b),
        "manifest_diff": manifest_diff,
        "metrics_diff": metrics_diff,
    });
    println!("{out}");
}

fn run_summary(run: &Run) -> serde_json::Value {
    serde_json::json!({
        "id": run.id.to_string(),
        "name": run.name,
        "vendor": run.vendor,
        "status": run.status.as_str(),
        "cost_usd": run.cost_usd,
        "created_at": run.created_at.to_rfc3339(),
        "duration_secs": duration_secs(run),
    })
}

fn duration_secs(run: &Run) -> Option<i64> {
    match (run.started_at, run.ended_at) {
        (Some(s), Some(e)) => Some((e - s).num_seconds()),
        _ => None,
    }
}

fn print_text(
    run_a: &Run,
    run_b: &Run,
    manifest_diff: &[ManifestDiffEntry],
    metrics_diff: &[MetricDiffEntry],
    args: &DiffArgs,
) {
    let id_a = run_a.id.to_string();
    let id_b = run_b.id.to_string();
    let short_a = &id_a[..id_a.len().min(8)];
    let short_b = &id_b[..id_b.len().min(8)];

    println!("a: {} ({})", short_a, run_a.name);
    println!(
        "   vendor={} status={} cost={} duration={}",
        run_a.vendor,
        run_a.status.as_str(),
        run_a
            .cost_usd
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "-".to_string()),
        fmt_duration(duration_secs(run_a)),
    );
    println!("b: {} ({})", short_b, run_b.name);
    println!(
        "   vendor={} status={} cost={} duration={}",
        run_b.vendor,
        run_b.status.as_str(),
        run_b
            .cost_usd
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "-".to_string()),
        fmt_duration(duration_secs(run_b)),
    );
    println!();

    if !args.metrics_only {
        println!("Manifest diff ({} differing paths):", manifest_diff.len());
        if manifest_diff.is_empty() {
            println!("  (identical)");
        } else {
            let path_w = manifest_diff
                .iter()
                .map(|e| e.path.len())
                .max()
                .unwrap_or(20)
                .max(20);
            println!("  {:<path_w$}  {:<24}  {:<24}", "path", "a", "b");
            for e in manifest_diff {
                println!(
                    "  {:<path_w$}  {:<24}  {:<24}",
                    e.path,
                    fmt_value(&e.a),
                    fmt_value(&e.b),
                );
            }
        }
        println!();
    }

    if !args.manifest_only {
        println!("Metrics diff ({} keys):", metrics_diff.len());
        if metrics_diff.is_empty() {
            println!("  (no metrics on either run)");
        } else {
            println!(
                "  {:<24}  {:<3}  {:<20}  {:<20}  {:<10}",
                "key", "dir", "a (last/best)", "b (last/best)", "Δ best"
            );
            for m in metrics_diff {
                println!(
                    "  {:<24}  {:<3}  {:<20}  {:<20}  {:<10}",
                    m.key,
                    m.direction,
                    fmt_pair(m.a_last, m.a_best),
                    fmt_pair(m.b_last, m.b_best),
                    fmt_delta(m.delta_best),
                );
            }
        }
    }
}

fn fmt_value(v: &Option<serde_json::Value>) -> String {
    match v {
        None => "—".to_string(),
        Some(x) => {
            let s = serde_json::to_string(x).unwrap_or_else(|_| "?".to_string());
            if s.len() > 24 {
                format!("{}…", &s[..23])
            } else {
                s
            }
        }
    }
}

fn fmt_pair(last: Option<f64>, best: Option<f64>) -> String {
    let l = last
        .map(|v| format!("{v:.4}"))
        .unwrap_or_else(|| "-".to_string());
    let b = best
        .map(|v| format!("{v:.4}"))
        .unwrap_or_else(|| "-".to_string());
    format!("{l} / {b}")
}

fn fmt_delta(d: Option<f64>) -> String {
    match d {
        None => "-".to_string(),
        Some(0.0) => "0.0000".to_string(),
        Some(v) if v > 0.0 => format!("+{v:.4}"),
        Some(v) => format!("{v:.4}"),
    }
}

fn fmt_duration(secs: Option<i64>) -> String {
    match secs {
        None => "-".to_string(),
        Some(s) if s < 60 => format!("{s}s"),
        Some(s) if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        Some(s) => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(s: &str) -> Yaml {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn manifest_diff_identical_yields_empty() {
        let a = yaml("name: foo\nrun:\n  cmd: python train.py\n");
        let b = yaml("name: foo\nrun:\n  cmd: python train.py\n");
        assert!(compute_manifest_diff(&a, &b).is_empty());
    }

    #[test]
    fn manifest_diff_scalar_change() {
        let a = yaml("run:\n  args:\n    --lr: 1.0e-3\n");
        let b = yaml("run:\n  args:\n    --lr: 5.0e-4\n");
        let d = compute_manifest_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "run.args.--lr");
        assert!(d[0].a.is_some() && d[0].b.is_some());
    }

    #[test]
    fn manifest_diff_added_and_removed_keys() {
        let a = yaml("name: foo\nvendor: vast\n");
        let b = yaml("name: foo\nvendor: kaggle\nnotes: hi\n");
        let d = compute_manifest_diff(&a, &b);
        // vendor changed + notes added
        assert_eq!(d.len(), 2);
        let paths: Vec<&str> = d.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"vendor"));
        assert!(paths.contains(&"notes"));
        let notes = d.iter().find(|e| e.path == "notes").unwrap();
        assert!(notes.a.is_none() && notes.b.is_some());
    }

    #[test]
    fn manifest_diff_sequence_index() {
        let a = yaml("data:\n  - src: x\n    dst: /a\n");
        let b = yaml("data:\n  - src: y\n    dst: /a\n  - src: z\n    dst: /b\n");
        let d = compute_manifest_diff(&a, &b);
        // data[0].src changed + data[1] added entirely
        let paths: Vec<&str> = d.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.iter().any(|p| p == &"data[0].src"));
        assert!(paths.iter().any(|p| p == &"data[1]"));
    }

    #[test]
    fn best_direction_loss_minimises() {
        assert_eq!(best_direction("val_loss"), "min");
        assert_eq!(best_direction("train_loss"), "min");
        assert_eq!(best_direction("rel_err"), "min");
        assert_eq!(best_direction("val_f1"), "max");
        assert_eq!(best_direction("accuracy"), "max");
    }

    #[test]
    fn best_direction_extended_names() {
        for k in [
            "mae",
            "val_rmse",
            "test/mse",
            "perplexity",
            "val_ppl",
            "wer",
            "cer",
            "fid",
            "val_MAE",
            "errors",
            "mseloss",
            "train.Loss",
            "top5_error",
        ] {
            assert_eq!(best_direction(k), "min", "{k}");
        }
        for k in [
            "accuracy",
            "f1",
            "val_f1",
            "reward",
            "auc",
            "iou",
            "bleu",
            "overall",
            "merge_rate",
            "kernel_score",
            "fidelity",
            "certainty",
            "terminal_reward",
        ] {
            assert_eq!(best_direction(k), "max", "{k}");
        }
    }

    /// Shared with python/xrun_tui/tests/test_metric_direction.py
    /// (`PARITY`): both sides must classify every key the same way.
    const PARITY: [(&str, &str); 38] = [
        ("val_loss", "min"),
        ("train.Loss", "min"),
        ("mseloss", "min"),
        ("error", "min"),
        ("top5_error", "min"),
        ("errors", "min"),
        ("mae", "min"),
        ("MAE", "min"),
        ("rmse", "min"),
        ("mse", "min"),
        ("perplexity", "min"),
        ("ppl", "min"),
        ("wer", "min"),
        ("cer", "min"),
        ("fid", "min"),
        ("valLoss", "min"),
        ("valError", "min"),
        ("valMAE", "min"),
        ("top5Error", "min"),
        ("FIDScore", "min"),
        ("nll", "min"),
        ("eer", "min"),
        ("bpb", "min"),
        ("bpc", "min"),
        // Known heuristic miss, pinned so both sides miss the same way (use
        // --direction / early_stop.mode): a weight, not a metric.
        ("loss_weight", "min"),
        ("lossless_ratio", "max"),
        ("losslessRatio", "max"),
        ("accuracy", "max"),
        ("val_f1", "max"),
        ("reward", "max"),
        ("fidelity", "max"),
        ("certainty", "max"),
        ("overall", "max"),
        ("merge_rate", "max"),
        ("kernel_score", "max"),
        ("terminal_reward", "max"),
        ("valFidelity", "max"),
        ("mAP", "max"),
    ];

    #[test]
    fn best_direction_parity_table() {
        for (k, want) in PARITY {
            assert_eq!(best_direction(k), want, "{k}");
        }
    }

    #[test]
    fn unknown_direction_keys_are_reported_sorted() {
        let o = overrides(&["zzz=min", "val_loss=max", "aaa=max"]);
        let keys = vec!["val_loss".to_string(), "acc".to_string()];
        assert_eq!(unknown_direction_keys(&o, &keys), vec!["aaa", "zzz"]);
        assert!(unknown_direction_keys(&overrides(&["acc=max"]), &keys).is_empty());
    }

    #[test]
    fn direction_flag_conflicting_repeat_is_an_error() {
        let o = overrides(&["a=min", "a=MIN"]);
        assert_eq!(o["a"], "min");
        let err = parse_direction_overrides(&["a=min".to_string(), "a=max".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("conflicting --direction for 'a'"), "{err}");
    }

    fn overrides(items: &[&str]) -> HashMap<String, &'static str> {
        let raw: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        parse_direction_overrides(&raw).unwrap()
    }

    #[test]
    fn direction_override_beats_early_stop_beats_heuristic() {
        let es_max = yaml("policy:\n  early_stop:\n    metric: val_loss\n    patience: 3\n");
        let es_min =
            yaml("policy:\n  early_stop:\n    metric: score\n    patience: 3\n    mode: min\n");
        let none = yaml("name: x\n");

        // heuristic only
        assert_eq!(
            resolve_direction("val_loss", &HashMap::new(), &[Some(&none)]),
            "min"
        );
        // early_stop (default mode max) beats the name heuristic
        assert_eq!(
            resolve_direction("val_loss", &HashMap::new(), &[Some(&es_max)]),
            "max"
        );
        // early_stop min beats heuristic max; B is consulted when A has none
        assert_eq!(
            resolve_direction("score", &HashMap::new(), &[Some(&none), Some(&es_min)]),
            "min"
        );
        // early_stop for a different metric is ignored
        assert_eq!(
            resolve_direction("val_loss", &HashMap::new(), &[Some(&es_min)]),
            "min"
        );
        // explicit override beats everything
        let o = overrides(&["score=max", "val_loss=max"]);
        assert_eq!(resolve_direction("score", &o, &[Some(&es_min)]), "max");
        assert_eq!(resolve_direction("val_loss", &o, &[Some(&none)]), "max");
    }

    #[test]
    fn direction_flag_accepts_values_and_rejects_bad_ones() {
        let o = overrides(&["a=min", " b = MAX "]);
        assert_eq!(o["a"], "min");
        assert_eq!(o["b"], "max");
        for bad in ["nokey", "a=up", "=min", "a="] {
            let err = parse_direction_overrides(&[bad.to_string()]).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("--direction"), "{msg}");
            assert!(msg.contains("min") && msg.contains("max"), "{msg}");
        }
    }
}
