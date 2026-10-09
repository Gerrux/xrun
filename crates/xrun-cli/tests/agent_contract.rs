use assert_cmd::Command;
use chrono::Utc;
use predicates::prelude::PredicateBooleanExt;
use serde_json::Value;
use tempfile::TempDir;
use xrun_core::{
    store::{NewEvent, RunStatus},
    vendor::InstanceHandle,
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
fn abbreviated_run_ids_resolve_and_ambiguous_stop_touches_nothing() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("data/runs.db")).unwrap();
    let a = store
        .create_run("a", "hash", "manifest", "local", &[])
        .unwrap();
    let b = store
        .create_run("b", "hash", "manifest", "local", &[])
        .unwrap();
    for id in [&a, &b] {
        store.update_run_status(id, RunStatus::Running).unwrap();
    }
    let (full_a, full_b) = (a.to_string(), b.to_string());

    // A unique, lower-cased tail resolves; JSON carries the full id.
    let output = command(&tmp)
        .args([
            "show",
            &full_a[full_a.len() - 10..].to_lowercase(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(shown["run"]["id"], full_a.as_str());

    // Both ids share the timestamp prefix: stop must refuse, not pick one.
    let shared: String = full_a
        .chars()
        .zip(full_b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect();
    assert!(shared.len() >= 4, "ids created together share a prefix");
    let output = command(&tmp)
        .args(["stop", &shared])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ambiguous (2 matches)"), "{stderr}");
    for id in [&a, &b] {
        assert_eq!(
            store.get_run(id).unwrap().unwrap().status,
            RunStatus::Running
        );
    }

    // Unknown id: the uniform message.
    command(&tmp)
        .args(["show", "ZZZZZZZZ"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "run not found: ZZZZZZZZ (see `xrun ls`)",
        ));
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

/// Seed a local run in `status` with a live (not destroyed) local instance
/// whose handle is saved, the way `launch` leaves it. The store is dropped
/// before the binary runs. Returns `(run_id, instance_id)`.
fn seed_run_with_live_instance(tmp: &TempDir, status: RunStatus) -> (String, String) {
    let mut store = Store::open(&tmp.path().join("data/runs.db")).unwrap();
    let run_id = store
        .create_run("kept", "hash", "manifest", "local", &[])
        .unwrap();
    store.update_run_status(&run_id, status).unwrap();
    let instance_id = format!("local-{run_id}");
    store
        .insert_instance(&instance_id, "local", Some(&run_id), None, None, Utc::now())
        .unwrap();
    let handle = InstanceHandle {
        id: instance_id.clone(),
        vendor: "local".into(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: String::new(),
        run_dir: None,
    };
    store
        .update_instance_state_json(&instance_id, &serde_json::to_string(&handle).unwrap())
        .unwrap();
    store.update_run_instance_id(&run_id, &instance_id).unwrap();
    (run_id.to_string(), instance_id)
}

#[test]
fn stop_on_finished_run_releases_instance_and_keeps_status() {
    // `policy.on_done: keep` leaves the instance alive after `done`; the
    // manual `xrun stop` that releases it must not rewrite a successful run
    // as `cancelled` (seen live on a Lightning Studio, 2026-10-09).
    let tmp = TempDir::new().unwrap();
    let (run_id, instance_id) = seed_run_with_live_instance(&tmp, RunStatus::Done);

    command(&tmp)
        .args(["stop", &run_id])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "released instance for finished run {run_id}"
        )))
        .stdout(predicates::str::contains("stopped").not());

    let store = Store::open(&tmp.path().join("data/runs.db")).unwrap();
    let run = store.get_run(&run_id.parse().unwrap()).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Done);
    let instance = store.get_instance(&instance_id).unwrap().unwrap();
    assert!(instance.destroyed_at.is_some(), "instance must be released");

    // Second stop: nothing left to release, status still untouched.
    command(&tmp)
        .args(["stop", &run_id])
        .assert()
        .success()
        .stdout(predicates::str::contains("already done"));
    let run = store.get_run(&run_id.parse().unwrap()).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Done);
}

#[test]
fn stop_on_running_run_destroys_instance_and_marks_cancelled() {
    let tmp = TempDir::new().unwrap();
    let (run_id, instance_id) = seed_run_with_live_instance(&tmp, RunStatus::Running);

    command(&tmp)
        .args(["stop", &run_id])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!("stopped {run_id}")));

    let store = Store::open(&tmp.path().join("data/runs.db")).unwrap();
    let run = store.get_run(&run_id.parse().unwrap()).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Cancelled);
    let instance = store.get_instance(&instance_id).unwrap().unwrap();
    assert!(
        instance.destroyed_at.is_some(),
        "instance must be destroyed"
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
