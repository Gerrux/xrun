use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

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
fn launch_without_dry_run_exits_one_with_not_implemented() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .arg("launch")
        .arg(manifest_path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not implemented"));
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
fn doctor_prints_check_and_status_columns() {
    let tmp = TempDir::new().unwrap();
    let out = xrun(&tmp).arg("doctor").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("check") && stdout.contains("status"),
        "doctor output missing table columns; got:\n{stdout}"
    );
}
