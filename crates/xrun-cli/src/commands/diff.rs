#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_yaml::Value as Yaml;
use xrun_core::{Run, RunId, Store, StoredMetric};

use crate::cli::DiffArgs;

pub fn run(args: &DiffArgs, db_path: &Path, runs_dir: &Path) -> Result<()> {
    let id_a: RunId = args
        .a
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.a))?;
    let id_b: RunId = args
        .b
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.b))?;

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let run_a = store
        .get_run(&id_a)
        .context("failed to query run a")?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", args.a))?;
    let run_b = store
        .get_run(&id_b)
        .context("failed to query run b")?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", args.b))?;

    let manifest_diff = if args.metrics_only {
        Vec::new()
    } else {
        let yaml_a = load_manifest_yaml(runs_dir, &run_a)?;
        let yaml_b = load_manifest_yaml(runs_dir, &run_b)?;
        compute_manifest_diff(&yaml_a, &yaml_b)
    };

    let metrics_diff = if args.manifest_only {
        Vec::new()
    } else {
        let key_filter: Option<Vec<String>> = args
            .keys
            .as_deref()
            .map(|s| s.split(',').map(str::trim).map(str::to_string).collect());
        compute_metrics_diff(&store, &id_a, &id_b, key_filter.as_deref())?
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
    /// "min" if the key looks loss-like, otherwise "max".
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

    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        let direction = best_direction(&key);
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

/// Heuristic: keys containing "loss", "err", or "error" minimise; everything
/// else maximises. Good enough for `val_loss` / `val_f1` / `accuracy` / `mae`.
/// `mae` and `mse` are intentionally treated as max-better here (rare and
/// users who care can pick keys explicitly); we only special-case the obvious
/// loss-like names.
pub fn best_direction(key: &str) -> &'static str {
    let k = key.to_ascii_lowercase();
    if k.contains("loss") || k.contains("err") {
        "min"
    } else {
        "max"
    }
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
}
