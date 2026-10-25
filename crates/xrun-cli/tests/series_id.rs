//! `series_id`: manifest field → runs row → `ls`/`show` JSON, inherited by
//! `rerun` and `sweep`. Local vendor only, isolated config/data dirs.

use assert_cmd::Command;
use chrono::Utc;
use serde_json::Value;
use tempfile::TempDir;

fn command(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("xrun").unwrap();
    cmd.current_dir(tmp.path())
        .env("XRUN_CONFIG_DIR", tmp.path().join("config"))
        .env("XRUN_DATA_DIR", tmp.path().join("data"))
        .timeout(std::time::Duration::from_secs(60));
    cmd
}

/// Local manifest that finishes `done` (same trick as `agent_contract.rs`).
fn write_manifest(tmp: &TempDir, file: &str, name: &str, series_id: Option<&str>) {
    let event = serde_json::json!({"ts": Utc::now().to_rfc3339(), "stage": "done", "status": "ok"})
        .to_string();
    let cmd = if cfg!(windows) {
        format!("[System.IO.File]::WriteAllText((Join-Path $env:XRUN_RUN_DIR 'events.jsonl'), '{event}' + [Environment]::NewLine)")
    } else {
        format!("printf '%s\\n' '{event}' > \"$XRUN_RUN_DIR/events.jsonl\"")
    };
    let mut m = serde_json::json!({"name": name, "vendor": "local", "run": {"cmd": cmd}});
    if let Some(s) = series_id {
        m["series_id"] = s.into();
    }
    std::fs::write(tmp.path().join(file), serde_yaml::to_string(&m).unwrap()).unwrap();
}

fn launch(tmp: &TempDir, args: &[&str]) -> String {
    let out = command(tmp)
        .arg("launch")
        .args(args)
        .args(["--json", "--allow-duplicate"])
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["run_id"].as_str().unwrap().to_string()
}

fn ls_all(tmp: &TempDir) -> Vec<Value> {
    let out = command(tmp)
        .args(["ls", "--all", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn row<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter().find(|r| r["id"] == id).expect("run in ls")
}

#[test]
fn launch_stores_series_id_and_ls_show_report_it() {
    let tmp = TempDir::new().unwrap();
    write_manifest(&tmp, "a.yaml", "with_series", Some("ab-48"));
    write_manifest(&tmp, "b.yaml", "no_series", None);
    let with = launch(&tmp, &["a.yaml"]);
    let without = launch(&tmp, &["b.yaml"]);

    let rows = ls_all(&tmp);
    assert_eq!(row(&rows, &with)["series_id"], "ab-48");
    let r = row(&rows, &without);
    assert!(r.get("series_id").is_some_and(Value::is_null), "row: {r}");

    let out = command(&tmp)
        .args(["show", &with, "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let show: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(show["run"]["series_id"], "ab-48");

    let frozen = std::fs::read_to_string(
        tmp.path()
            .join("data/runs")
            .join(&with)
            .join("manifest.yaml"),
    )
    .unwrap();
    assert!(frozen.contains("series_id: ab-48"), "frozen:\n{frozen}");
}

#[test]
fn override_sets_series_id_and_rerun_inherits_it() {
    let tmp = TempDir::new().unwrap();
    write_manifest(&tmp, "a.yaml", "ov", None);
    let first = launch(&tmp, &["a.yaml", "--override", "series_id=from-override"]);

    command(&tmp).args(["rerun", &first]).assert().success();
    let rows = ls_all(&tmp);
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert_eq!(r["series_id"], "from-override", "row: {r}");
    }
}

#[test]
fn override_with_invalid_series_id_is_rejected() {
    let tmp = TempDir::new().unwrap();
    write_manifest(&tmp, "a.yaml", "ov", None);
    command(&tmp)
        .args([
            "launch",
            "a.yaml",
            "--dry-run",
            "--override",
            "series_id=Bad_One",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("series_id must match"));
}

#[test]
fn invalid_series_id_shows_in_dry_run_and_doctor() {
    let tmp = TempDir::new().unwrap();
    write_manifest(&tmp, "bad.yaml", "bad", Some("Not_A_Slug"));

    command(&tmp)
        .args(["launch", "bad.yaml", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("series_id must match"));

    let out = command(&tmp)
        .args(["doctor", "--manifest", "bad.yaml", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("series_id must match"), "doctor:\n{stdout}");
}

fn sweep_series(tmp: &TempDir, out_dir: &str) -> Vec<Option<String>> {
    command(tmp)
        .args([
            "sweep",
            "base.yaml",
            "--grid",
            // `run.args` is null in the base, and the patcher cannot descend
            // into null — a top-level axis keeps this about series_id.
            "description=a,b",
            "--out",
            out_dir,
        ])
        .assert()
        .success();
    let mut files: Vec<_> = std::fs::read_dir(tmp.path().join(out_dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2);
    files
        .iter()
        .map(|p| {
            let m: Value = serde_yaml::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
            m["series_id"].as_str().map(str::to_string)
        })
        .collect()
}

#[test]
fn sweep_inherits_base_series_id() {
    let tmp = TempDir::new().unwrap();
    write_manifest(&tmp, "base.yaml", "my_exp", Some("campaign-1"));
    let series = sweep_series(&tmp, "out");
    assert!(series.iter().all(|s| s.as_deref() == Some("campaign-1")));
}

#[test]
fn sweep_without_base_series_id_derives_one_from_name_and_timestamp() {
    let tmp = TempDir::new().unwrap();
    write_manifest(&tmp, "base.yaml", "my_exp", None);
    let series = sweep_series(&tmp, "out");
    let first = series[0].clone().expect("series_id set");
    assert!(series.iter().all(|s| s.as_deref() == Some(first.as_str())));
    // my-exp-YYYYMMDD-HHMMSS
    let rest = first.strip_prefix("my-exp-").expect(&first);
    let (date, time) = rest.split_once('-').expect(&first);
    assert!(
        date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()),
        "{first}"
    );
    assert!(
        time.len() == 6 && time.chars().all(|c| c.is_ascii_digit()),
        "{first}"
    );
}
