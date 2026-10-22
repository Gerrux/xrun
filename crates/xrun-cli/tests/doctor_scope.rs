//! `xrun doctor --manifest` checks only the named manifests; without it the
//! exp/ directory is scanned. Local manifests only — nothing here touches the
//! network or a vendor binary.

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const OK_YAML: &str = "name: ok\nvendor: local\nrun:\n  cmd: echo hi\n";
const BROKEN_YAML: &str = "name: [unterminated\nvendor: : :\n";
// Valid, but names a vendor the creds-less temp config does not activate:
// only a manifest that is actually checked may bring `kaggle_binary` in.
const KAGGLE_YAML: &str =
    "name: kg\nvendor: kaggle\nkaggle:\n  kernel_slug: tester/kg\nrun:\n  cmd: echo hi\n";

fn project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let exp = tmp.path().join("exp");
    std::fs::create_dir_all(&exp).unwrap();
    std::fs::write(exp.join("ok.yaml"), OK_YAML).unwrap();
    std::fs::write(exp.join("broken.yaml"), BROKEN_YAML).unwrap();
    std::fs::write(exp.join("kg.yaml"), KAGGLE_YAML).unwrap();
    tmp
}

fn doctor(tmp: &TempDir, args: &[&str]) -> (Vec<Value>, Option<i32>) {
    let out = Command::cargo_bin("xrun")
        .unwrap()
        .current_dir(tmp.path())
        .env("XRUN_CONFIG_DIR", tmp.path().join("config"))
        .env("XRUN_DATA_DIR", tmp.path().join("data"))
        .arg("doctor")
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    let rows: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "doctor --json is not JSON ({e}); stdout:\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (rows, out.status.code())
}

fn details<'a>(rows: &'a [Value], check: &str) -> Vec<&'a str> {
    rows.iter()
        .filter(|r| r["check"] == check)
        .map(|r| r["detail"].as_str().unwrap())
        .collect()
}

#[test]
fn manifest_flag_checks_only_the_named_file() {
    let tmp = project();
    let (rows, _) = doctor(&tmp, &["--manifest", "exp/ok.yaml"]);

    let manifests = details(&rows, "manifest");
    assert_eq!(manifests.len(), 1, "rows: {manifests:?}");
    assert!(manifests[0].contains("ok.yaml"), "row: {}", manifests[0]);
    assert!(
        rows.iter()
            .all(|r| !r["detail"].as_str().unwrap().contains("broken.yaml")),
        "broken.yaml leaked into the report: {rows:?}"
    );
    // The other manifest's vendor must not activate its checks either.
    assert!(
        details(&rows, "kaggle_binary").is_empty(),
        "kaggle checks ran for a manifest that was not named: {rows:?}"
    );
}

#[test]
fn without_manifest_flag_exp_is_scanned() {
    let tmp = project();
    let (rows, _) = doctor(&tmp, &[]);

    let manifests = details(&rows, "manifest");
    assert_eq!(manifests.len(), 3, "rows: {manifests:?}");
    let broken: Vec<_> = rows
        .iter()
        .filter(|r| r["check"] == "manifest" && r["detail"].as_str().unwrap().contains("broken"))
        .collect();
    assert_eq!(broken.len(), 1);
    // A broken auto-discovered manifest is advisory, not a failure.
    assert_eq!(broken[0]["status"], "WARN");
    assert!(
        !details(&rows, "kaggle_binary").is_empty(),
        "the scanned kaggle manifest should activate the kaggle vendor: {rows:?}"
    );
}

#[test]
fn explicit_broken_manifest_is_fatal() {
    let tmp = project();
    let (rows, code) = doctor(&tmp, &["--manifest", "exp/broken.yaml"]);

    let manifests: Vec<_> = rows.iter().filter(|r| r["check"] == "manifest").collect();
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0]["status"], "FAIL");
    assert_eq!(code, Some(1));
}

#[test]
fn same_manifest_spelled_twice_is_checked_once() {
    let tmp = project();
    let (rows, _) = doctor(
        &tmp,
        &["--manifest", "exp/ok.yaml", "--manifest", "./exp/ok.yaml"],
    );
    assert_eq!(details(&rows, "manifest").len(), 1, "rows: {rows:?}");
}
