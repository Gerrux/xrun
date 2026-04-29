#![deny(unsafe_code)]

//! Manifest patches shared by `xrun launch --override` and `xrun rerun --patch`.
//!
//! Syntax: `path.dotted=value`. The path walks through nested objects in the
//! serialized manifest. Values are interpreted as JSON if they parse as such
//! (`5e-4` → number, `true` → bool), else as a literal string. The most common
//! case is `run.args.<key>=<value>` to flip a single hyperparameter.

use anyhow::{Context, Result};
use serde_json::Value;
use xrun_core::manifest::Manifest;

/// Apply a list of `path=value` overrides to a manifest in-place. Returns the
/// patched manifest, leaving the input untouched on error.
pub fn apply(manifest: &Manifest, overrides: &[String]) -> Result<Manifest> {
    if overrides.is_empty() {
        return Ok(manifest.clone());
    }
    let mut value = serde_json::to_value(manifest).context("manifest -> JSON failed")?;
    for raw in overrides {
        let (path, val) = raw
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--override expects PATH=VALUE, got: {raw}"))?;
        if path.is_empty() {
            anyhow::bail!("--override has empty path: {raw}");
        }
        let parsed = parse_value(val);
        set_path(&mut value, path, parsed)
            .with_context(|| format!("failed to apply override {raw}"))?;
    }
    serde_json::from_value(value).context(
        "patched manifest no longer validates — check the override paths and value types",
    )
}

/// Try JSON first (so `5e-4`, `true`, `[1,2]`, `null` round-trip as their
/// native types), fall back to a literal string.
fn parse_value(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

fn set_path(root: &mut Value, path: &str, leaf: Value) -> Result<()> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut cur = root;
    for seg in &segments[..segments.len() - 1] {
        if !cur.is_object() {
            anyhow::bail!("path segment '{seg}' descends into a non-object");
        }
        let map = cur.as_object_mut().expect("checked above");
        // Auto-vivify intermediate objects so `--override foo.bar.baz=1` works
        // when foo.bar didn't exist yet.
        cur = map
            .entry((*seg).to_string())
            .or_insert_with(|| Value::Object(Default::default()));
    }
    let last = segments.last().expect("path is non-empty");
    let map = cur
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("cannot set leaf '{last}' on non-object"))?;
    map.insert((*last).to_string(), leaf);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_manifest() -> Manifest {
        let yaml = r#"
name: test
vendor: vast
vast:
  image: pytorch/pytorch:latest
  gpu:
    type: RTX_4090
    count: 1
run:
  cmd: python train.py
  args:
    --lr: 1e-3
"#;
        Manifest::from_yaml_str(yaml).unwrap()
    }

    #[test]
    fn override_run_arg_replaces_value() {
        let m = base_manifest();
        let patched = apply(&m, &["run.args.--lr=5e-4".to_string()]).unwrap();
        let lr = patched
            .run
            .args
            .as_ref()
            .unwrap()
            .get("--lr")
            .unwrap()
            .clone();
        assert_eq!(lr, serde_json::json!(5e-4));
    }

    #[test]
    fn override_string_value_falls_back_to_string() {
        let m = base_manifest();
        let patched = apply(&m, &["name=experiment-2".to_string()]).unwrap();
        assert_eq!(patched.name, "experiment-2");
    }

    #[test]
    fn override_invalid_path_returns_error() {
        let m = base_manifest();
        let err = apply(&m, &["run.args.--lr".to_string()]).unwrap_err();
        assert!(err.to_string().contains("PATH=VALUE"));
    }
}
