#![deny(unsafe_code)]

//! Inject xrun_hook + MLFLOW_* into a user-supplied `.ipynb` so notebook-mode
//! kernels get the same live-telemetry plumbing as script-mode.
//!
//! Why this exists: Kaggle script-mode kernels strip every sibling file from
//! the push, so we base64-embed the xrun_hook wheel directly into the
//! generated `main.py` and pip-install it from a prelude block (see
//! `build_script_main` in `adapter.rs`). Notebook-mode keeps the user's
//! `.ipynb` intact, so we have to do the same trick at the notebook level —
//! prepend one synthetic code cell that decodes + installs the wheel and
//! exports the MLFLOW_* env vars before any user cell runs.
//!
//! Without this prelude cell, `xrun_hook.metric()` calls inside the kernel
//! silently fail to reach MLflow because `MLFLOW_TRACKING_URI` is unset, and
//! `xrun events <id>` / `xrun metrics <id>` show only the host-side queue/
//! running events for the entire run.

use serde_json::{json, Value};

/// Marker stored on the prelude cell so we can identify it later (e.g. for
/// idempotent re-injection if the user rebuilds against an updated wheel).
pub const PRELUDE_CELL_TAG: &str = "xrun-bootstrap";

/// Read a `.ipynb` file, prepend an xrun-bootstrap code cell, and return the
/// modified notebook as a serialized JSON string ready to drop into staging.
///
/// `env_prelude` is the same `os.environ['…'] = '…'` block produced by
/// `build_env_prelude` in adapter.rs — empty when MLflow isn't configured.
/// `wheel_b64` is the base64-encoded xrun_hook wheel — None when no wheel
/// was embedded at build time.
///
/// When *both* are absent the notebook is returned unchanged (no point
/// prepending an empty cell). The error path covers parser failures only;
/// missing wheel/env is a normal config and not an error.
pub fn inject_bootstrap_cell(
    notebook_json: &str,
    env_prelude: &str,
    wheel_b64: Option<&str>,
) -> Result<String, NotebookInjectError> {
    let mut nb: Value = serde_json::from_str(notebook_json)
        .map_err(|e| NotebookInjectError::Parse(e.to_string()))?;

    // No-op when there's nothing to inject — keeps the user's notebook
    // byte-identical, and avoids prepending a synthetic cell that does
    // literally nothing (which would surface as a confusing empty cell at
    // the top of the kernel viewer on Kaggle).
    if env_prelude.is_empty() && wheel_b64.is_none() {
        return Ok(notebook_json.to_string());
    }

    let cell_source = build_prelude_source(env_prelude, wheel_b64);

    // ipynb spec allows `source` as either a string or an array of strings;
    // we always emit the array form so newline boundaries match how Jupyter
    // displays the cell.
    let source_lines: Vec<Value> = cell_source
        .split_inclusive('\n')
        .map(|line| Value::String(line.to_string()))
        .collect();

    let cell = json!({
        "cell_type": "code",
        "execution_count": null,
        "metadata": {
            "tags": [PRELUDE_CELL_TAG],
            "xrun_generated": true,
        },
        "outputs": [],
        "source": source_lines,
    });

    let cells = nb
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| NotebookInjectError::Parse("notebook missing 'cells' array".to_string()))?;
    cells.insert(0, cell);

    serde_json::to_string(&nb).map_err(|e| NotebookInjectError::Parse(e.to_string()))
}

/// Build the Python source for the bootstrap cell. Public for unit tests in
/// the same crate; not part of the external API.
pub fn build_prelude_source(env_prelude: &str, wheel_b64: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str("# === xrun bootstrap (auto-generated, do not edit) ===\n");
    out.push_str("# Sets MLFLOW_* env vars and installs xrun_hook so the kernel can\n");
    out.push_str("# stream events/metrics back to xrun while running. Removing this\n");
    out.push_str("# cell breaks live telemetry; xrun events/metrics will be empty.\n");
    out.push_str("import os, sys, subprocess as _sp\n");

    if !env_prelude.is_empty() {
        out.push_str(env_prelude);
        if !env_prelude.ends_with('\n') {
            out.push('\n');
        }
    }

    if let Some(b64) = wheel_b64 {
        out.push_str("import base64 as _b64, tempfile as _tf\n");
        out.push_str("_WHEEL_B64 = '''");
        out.push_str(b64);
        out.push_str("'''\n");
        // pip rejects wheels whose filename doesn't match the canonical
        // `<pkg>-<ver>-<py>-<abi>-<plat>.whl` convention, so write into a
        // temp dir under the canonical name rather than mktemp's random one.
        out.push_str("_whl_dir = _tf.mkdtemp(prefix='xrun_hook_')\n");
        out.push_str("_whl_path = os.path.join(_whl_dir, 'xrun_hook-0.0.0-py3-none-any.whl')\n");
        out.push_str("with open(_whl_path, 'wb') as _f:\n");
        out.push_str("    _f.write(_b64.b64decode(_WHEEL_B64))\n");
        out.push_str(
            "_r = _sp.run([sys.executable, '-m', 'pip', 'install', '--quiet', \
             '--no-deps', _whl_path], capture_output=True, text=True)\n",
        );
        out.push_str("if _r.returncode != 0:\n");
        out.push_str("    print('xrun_hook bootstrap failed:', _r.stderr, flush=True)\n");
        out.push_str("else:\n");
        out.push_str("    try:\n");
        out.push_str("        import xrun_hook  # noqa: F401  starts streamer\n");
        out.push_str("    except Exception as _e:\n");
        out.push_str("        print('xrun_hook import failed:', _e, flush=True)\n");
        // Block subprocess re-imports of xrun_hook from creating a second
        // streamer (which would push duplicate chunks to a separate MLflow
        // run). User cells can still call xrun_hook.metric/.epoch/.done —
        // only the log streamer is suppressed downstream.
        out.push_str("os.environ['XRUN_LOG_STREAM_DISABLE'] = '1'\n");
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum NotebookInjectError {
    #[error("notebook parse failed: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_notebook() -> String {
        r#"{
            "cells": [
                {"cell_type": "code", "metadata": {}, "source": ["print('hi')\n"], "execution_count": null, "outputs": []}
            ],
            "metadata": {"kernelspec": {"name": "python3"}},
            "nbformat": 4,
            "nbformat_minor": 5
        }"#
        .to_string()
    }

    #[test]
    fn passthrough_when_nothing_to_inject() {
        let nb = empty_notebook();
        let out = inject_bootstrap_cell(&nb, "", None).unwrap();
        assert_eq!(out, nb);
    }

    #[test]
    fn prepends_cell_when_env_set() {
        let nb = empty_notebook();
        let env = "os.environ['MLFLOW_TRACKING_URI'] = 'http://x'\n";
        let out = inject_bootstrap_cell(&nb, env, None).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let cells = v["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 2, "expected one prepended cell");
        let first = &cells[0];
        assert_eq!(first["cell_type"].as_str().unwrap(), "code");
        let source: String = first["source"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(source.contains("MLFLOW_TRACKING_URI"));
        assert!(
            !source.contains("_WHEEL_B64"),
            "wheel block must be skipped when no wheel"
        );
        // User cell still intact and in original position
        assert_eq!(
            cells[1]["source"].as_array().unwrap()[0].as_str().unwrap(),
            "print('hi')\n"
        );
    }

    #[test]
    fn prepends_cell_when_wheel_present() {
        let nb = empty_notebook();
        let out = inject_bootstrap_cell(&nb, "", Some("AAAA")).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let source: String = v["cells"][0]["source"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(source.contains("_WHEEL_B64 = '''AAAA'''"));
        assert!(source.contains("pip"));
        assert!(source.contains("import xrun_hook"));
        assert!(source.contains("XRUN_LOG_STREAM_DISABLE"));
    }

    #[test]
    fn cell_is_tagged_for_recognition() {
        let nb = empty_notebook();
        let out = inject_bootstrap_cell(&nb, "x=1\n", None).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let tags = v["cells"][0]["metadata"]["tags"].as_array().unwrap();
        assert_eq!(tags[0].as_str().unwrap(), PRELUDE_CELL_TAG);
        assert!(v["cells"][0]["metadata"]["xrun_generated"]
            .as_bool()
            .unwrap());
    }

    #[test]
    fn missing_cells_array_is_an_error() {
        let bad = r#"{"metadata": {}, "nbformat": 4}"#;
        let err = inject_bootstrap_cell(bad, "x=1\n", None).unwrap_err();
        assert!(matches!(err, NotebookInjectError::Parse(_)));
    }

    #[test]
    fn invalid_json_is_an_error() {
        let err = inject_bootstrap_cell("{not json", "x=1\n", None).unwrap_err();
        assert!(matches!(err, NotebookInjectError::Parse(_)));
    }
}
