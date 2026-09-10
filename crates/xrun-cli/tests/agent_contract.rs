use assert_cmd::Command;
use chrono::Utc;
use serde_json::Value;
use tempfile::TempDir;
use xrun_core::{
    store::{NewEvent, RunStatus},
    Store,
};

fn command(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("xrun").unwrap();
    cmd.current_dir(tmp.path())
        .env("XRUN_CONFIG_DIR", tmp.path().join("config"))
        .env("XRUN_DATA_DIR", tmp.path().join("data"));
    cmd
}

fn manifest(tmp: &TempDir, script: &str) {
    let event = serde_json::json!({"ts": Utc::now().to_rfc3339(), "stage": "done", "status": "ok"})
        .to_string();
    let cmd = if cfg!(windows) {
        format!("[System.IO.File]::WriteAllText((Join-Path $env:XRUN_RUN_DIR 'events.jsonl'), '{event}' + [Environment]::NewLine)")
    } else {
        format!("printf '%s\\n' '{event}' > \"$XRUN_RUN_DIR/events.jsonl\"")
    };
    let mut manifest = serde_json::json!({"name": "smoke", "vendor": "local", "run": {"cmd": cmd}});
    if script == "exit 7" {
        manifest["run"]["setup"] = "exit 7".into();
    }
    std::fs::write(
        tmp.path().join("smoke.yaml"),
        serde_yaml::to_string(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn foreground_launch_returns_one_json_result() {
    let tmp = TempDir::new().unwrap();
    manifest(&tmp, "echo smoke");
    let output = command(&tmp)
        .args(["launch", "smoke.yaml", "--json"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .get_output()
        .clone();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "done");
    assert!(result["run_id"]
        .as_str()
        .unwrap()
        .parse::<xrun_core::RunId>()
        .is_ok());
    assert!(result["instance_id"].is_string());
}

#[test]
fn detached_launch_returns_json_and_daemon_finishes() {
    let tmp = TempDir::new().unwrap();
    manifest(&tmp, "echo detached");
    let output = command(&tmp)
        .arg("--config-dir")
        .arg(tmp.path().join("explicit-config"))
        .args(["launch", "smoke.yaml", "--detach", "--json"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .get_output()
        .clone();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "running");
    assert!(result["poller_pid"].as_u64().unwrap() > 0);
    let id: xrun_core::RunId = result["run_id"].as_str().unwrap().parse().unwrap();
    let store = Store::open(&tmp.path().join("data/runs.db")).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let status = store.get_run(&id).unwrap().unwrap().status;
        if status == RunStatus::Done {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "daemon did not finish: {status:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(tmp
        .path()
        .join("data/runs")
        .join(id.to_string())
        .join("poller.log")
        .exists());
}

#[test]
fn launch_validation_error_is_json() {
    let tmp = TempDir::new().unwrap();
    let output = command(&tmp)
        .args(["launch", "missing.yaml", "--json"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "launch_failed");
}

#[test]
fn events_follow_json_is_jsonl_without_table_headers() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("data/runs.db")).unwrap();
    let id = store
        .create_run("test", "hash", "manifest", "local", &[])
        .unwrap();
    for stage in ["train_start", "done"] {
        store
            .append_event(
                &id,
                NewEvent {
                    ts: Utc::now(),
                    stage: stage.into(),
                    status: "ok".into(),
                    msg: None,
                    payload_json: None,
                },
            )
            .unwrap();
    }
    store.update_run_status(&id, RunStatus::Done).unwrap();
    let output = command(&tmp)
        .args(["events", &id.to_string(), "--follow", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["stage"], "done");
}

#[test]
fn sweep_failure_is_valid_json_and_nonzero_exit() {
    let tmp = TempDir::new().unwrap();
    manifest(&tmp, "exit 7");
    let output = command(&tmp)
        .args([
            "sweep",
            "smoke.yaml",
            "--grid",
            "name=a,b",
            "--launch",
            "--json",
        ])
        .timeout(std::time::Duration::from_secs(60))
        .assert()
        .failure()
        .get_output()
        .clone();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let runs = result["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs
        .iter()
        .all(|r| r["success"] == false && r["error"].is_string()));
}

#[test]
fn keep_instance_does_not_falsely_mark_run_cancelled() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("data/runs.db")).unwrap();
    let id = store
        .create_run("test", "hash", "manifest", "local", &[])
        .unwrap();
    store.update_run_status(&id, RunStatus::Running).unwrap();
    command(&tmp)
        .args(["stop", &id.to_string(), "--keep-instance"])
        .assert()
        .failure();
    assert_eq!(
        store.get_run(&id).unwrap().unwrap().status,
        RunStatus::Running
    );
}

#[test]
fn skill_upgrade_preserves_user_instructions_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let legacy = "Before\n<!-- xrun-skill -->\n# xrun Skill\n\nWhen working with ML experiment runs in this repository, use the project skill at `.codex/skills/xrun/SKILL.md`.\nAfter\n";
    std::fs::write(tmp.path().join("AGENTS.md"), legacy).unwrap();
    for _ in 0..2 {
        command(&tmp)
            .args(["install", "skill", "--codex"])
            .assert()
            .success();
    }
    let instructions = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(instructions.starts_with("Before\n"));
    assert!(instructions.ends_with("\nAfter\n"));
    assert_eq!(instructions.matches("<!-- xrun-skill -->").count(), 1);
    assert!(instructions.contains(".agents/skills/xrun/SKILL.md"));
    let skill = std::fs::read_to_string(tmp.path().join(".agents/skills/xrun/SKILL.md")).unwrap();
    let metadata: serde_yaml::Value =
        serde_yaml::from_str(skill.split("---").nth(1).unwrap()).unwrap();
    assert_eq!(metadata["name"].as_str(), Some("xrun"));
    assert!(!metadata["description"].as_str().unwrap().is_empty());
}
