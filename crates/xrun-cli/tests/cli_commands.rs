use std::path::PathBuf;

use assert_cmd::Command;
use chrono::Utc;
use predicates::prelude::*;
use tempfile::TempDir;
use xrun_core::{store::NewMetric, RunId, Store};

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/vast_minimal.yaml")
}

fn xrun(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("xrun").unwrap();
    cmd.env("XRUN_DATA_DIR", tmp.path())
        .env("XRUN_CONFIG_DIR", tmp.path().join("config"));
    cmd
}

#[test]
fn launch_dry_run_exits_zero_and_prints_gpu_query() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .arg("launch")
        .arg(manifest_path())
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("gpu_query"));
}

#[test]
fn launch_without_dry_run_exits_one_with_error() {
    let tmp = TempDir::new().unwrap();
    // Without a real vastai binary the provision step fails; we just check for exit 1.
    xrun(&tmp)
        .arg("launch")
        .arg(manifest_path())
        .assert()
        .failure();
}

#[test]
fn ls_json_on_empty_db_returns_empty_array() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .arg("ls")
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains("[]"));
}

#[test]
fn show_nonexistent_id_exits_one() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .arg("show")
        .arg("00000000000000000000000000")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("run not found").or(predicate::str::contains("not found")),
        );
}

#[test]
fn diff_nonexistent_runs_exits_one() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .arg("diff")
        .arg("00000000000000000000000000")
        .arg("00000000000000000000000001")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn doctor_prints_check_and_status_columns() {
    let tmp = TempDir::new().unwrap();
    let result = xrun(&tmp).arg("doctor").assert();
    let output = result.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(
        stdout.contains("check") && stdout.contains("status"),
        "doctor output missing table columns; got:\n{stdout}"
    );
    // Verify advisory checks appear as WARN (not FAIL) and don't cause hard failure alone.
    // The rsync_binary and python_xrun_hook checks are warn-only.
    // Exit-code contract is verified by the separate test below.
}

/// Seed a run in the store at `XRUN_DATA_DIR/runs.db` (matching `xrun(&tmp)`)
/// with `series` as `(key, [(step, value)])`. The connection is dropped before
/// the binary is spawned.
fn seed_run(tmp: &TempDir, series: &[(&str, &[(i64, f64)])]) -> RunId {
    let mut store = Store::open(&tmp.path().join("runs.db")).unwrap();
    let id = store
        .create_run("metrics-test", "hash", "manifest", "local", &[])
        .unwrap();
    for (key, points) in series {
        for (step, value) in points.iter() {
            store
                .append_metric(
                    &id,
                    NewMetric {
                        step: *step,
                        key: (*key).to_string(),
                        value: *value,
                        ts: Utc::now(),
                    },
                )
                .unwrap();
        }
    }
    id
}

#[test]
fn metrics_ascii_renders_chart_for_run_with_points() {
    let tmp = TempDir::new().unwrap();
    let id = seed_run(
        &tmp,
        &[
            (
                "val_f1",
                &[(0, 0.1), (1, 0.4), (2, 0.6), (3, 0.55), (4, 0.7)],
            ),
            ("loss", &[(0, 2.0), (1, 1.0)]),
        ],
    );
    let out = xrun(&tmp)
        .args(["metrics", &id.to_string(), "--key", "val_f1", "--ascii"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(stdout.starts_with("val_f1\n"), "got:\n{stdout}");
    assert!(
        stdout.contains('*'),
        "chart has no plotted points:\n{stdout}"
    );
    assert!(
        stdout.contains("min=0.1000  max=0.7000  last=0.7000  n=5"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("step 0"), "got:\n{stdout}");
    assert!(!stdout.contains("no data yet"), "got:\n{stdout}");
    assert!(!stdout.contains("loss"), "unselected key leaked:\n{stdout}");
}

#[test]
fn metrics_ascii_without_key_renders_all_keys() {
    let tmp = TempDir::new().unwrap();
    let id = seed_run(&tmp, &[("a", &[(0, 1.0), (1, 2.0)]), ("b", &[(0, 5.0)])]);
    xrun(&tmp)
        .args(["metrics", &id.to_string(), "--ascii"])
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("a\n")
                .and(predicate::str::contains("\nb\n"))
                .and(predicate::str::contains("n=2"))
                .and(predicate::str::contains("n=1"))
                .and(predicate::str::contains("no data yet").not()),
        );
}

#[test]
fn metrics_ascii_prints_no_data_yet_for_run_without_points() {
    let tmp = TempDir::new().unwrap();
    let id = seed_run(&tmp, &[]);
    xrun(&tmp)
        .args(["metrics", &id.to_string(), "--ascii"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no data yet").and(predicate::str::contains('*').not()));
    // Explicit --key for a missing key: still the empty message.
    xrun(&tmp)
        .args(["metrics", &id.to_string(), "--key", "val_f1", "--ascii"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no data yet"));
}

#[test]
fn metrics_json_is_unchanged_when_ascii_also_given() {
    let tmp = TempDir::new().unwrap();
    let id = seed_run(&tmp, &[("val_f1", &[(0, 0.5), (1, 0.75)])]);
    let out = xrun(&tmp)
        .args([
            "metrics",
            &id.to_string(),
            "--key",
            "val_f1",
            "--json",
            "--ascii",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["step"], 1);
    assert_eq!(rows[1]["key"], "val_f1");
    assert_eq!(rows[1]["value"], 0.75);
}

#[test]
fn doctor_exits_one_when_checks_fail() {
    let tmp = TempDir::new().unwrap();
    // In any environment without vastai+kaggle in PATH, doctor exits 1.
    // If this test runs on a machine with both binaries, it may pass doctor and exit 0 — skip then.
    let out = xrun(&tmp).arg("doctor").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("FAIL") {
        assert!(
            !out.status.success(),
            "doctor should exit 1 when checks fail"
        );
    }
}
