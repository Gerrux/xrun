//! `fix-status` / `resume` close `vendor: local` runs whose training process
//! is gone (host restart, crash) instead of trusting that the instance row is
//! merely present. All state lives in a temp `XRUN_CONFIG_DIR`/`XRUN_DATA_DIR`.

use assert_cmd::Command;
use chrono::Utc;
use serde_json::Value;
use tempfile::TempDir;
use xrun_core::{
    store::{RunId, RunStatus},
    vendor::InstanceHandle,
    Store,
};

/// Never a live PID on any supported platform.
const DEAD_PID: u32 = u32::MAX - 1;

fn command(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("xrun").unwrap();
    cmd.current_dir(tmp.path())
        .env("XRUN_CONFIG_DIR", tmp.path().join("config"))
        .env("XRUN_DATA_DIR", tmp.path().join("data"));
    cmd
}

fn open(tmp: &TempDir) -> Store {
    Store::open(&tmp.path().join("data/runs.db")).unwrap()
}

fn run_dir(tmp: &TempDir, id: &RunId) -> std::path::PathBuf {
    tmp.path().join("data/runs").join(id.to_string())
}

/// A `running` local run with its instance row and `run.pid = pid`.
/// `pid_age_secs` backdates the pid file's mtime (the PID-reuse cutoff).
fn seed(tmp: &TempDir, pid: u32, pid_age_secs: u64, done_ok_in_events_file: bool) -> RunId {
    let mut store = open(tmp);
    let id = store
        .create_run("stale", "hash", "manifest.yaml", "local", &[])
        .unwrap();
    let instance_id = format!("local-{id}");
    store
        .insert_instance(&instance_id, "local", Some(&id), None, None, Utc::now())
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
    store.update_run_instance_id(&id, &instance_id).unwrap();
    store.update_run_started_at(&id, Utc::now()).unwrap();
    store.update_run_status(&id, RunStatus::Running).unwrap();

    let dir = run_dir(tmp, &id);
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("run.pid");
    std::fs::write(&pid_file, pid.to_string()).unwrap();
    if pid_age_secs > 0 {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(pid_age_secs);
        std::fs::File::options()
            .write(true)
            .open(&pid_file)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }
    if done_ok_in_events_file {
        let ev =
            serde_json::json!({"ts": Utc::now().to_rfc3339(), "stage": "done", "status": "ok"});
        std::fs::write(dir.join("events.jsonl"), format!("{ev}\n")).unwrap();
    }
    id
}

fn status_of(tmp: &TempDir, id: &RunId) -> RunStatus {
    open(tmp).get_run(id).unwrap().unwrap().status
}

fn instance_destroyed(tmp: &TempDir, id: &RunId) -> bool {
    open(tmp)
        .get_instance(&format!("local-{id}"))
        .unwrap()
        .unwrap()
        .destroyed_at
        .is_some()
}

/// Whether the start time of this very process is readable here; the
/// recycled-PID cases are only decidable where it is (linux, windows, macOS).
fn start_time_readable() -> bool {
    xrun_local::process::process_started_at(std::process::id()).is_some()
}

#[test]
fn fix_status_fails_a_local_run_whose_process_died() {
    let tmp = TempDir::new().unwrap();
    let id = seed(&tmp, DEAD_PID, 0, false);
    command(&tmp)
        .args(["fix-status", &id.to_string()])
        .assert()
        .success();
    assert_eq!(status_of(&tmp, &id), RunStatus::Failed);
    assert!(instance_destroyed(&tmp, &id));
    let events = open(&tmp).list_events(&id).unwrap();
    let failed = events
        .iter()
        .find(|e| e.stage == "stage_failed" && e.status == "fail")
        .expect("stage_failed event");
    assert!(failed.msg.as_deref().unwrap().contains("without done:ok"));
}

#[test]
fn fix_status_marks_done_when_events_file_has_done_ok() {
    let tmp = TempDir::new().unwrap();
    let id = seed(&tmp, DEAD_PID, 0, true);
    command(&tmp)
        .args(["fix-status", &id.to_string()])
        .assert()
        .success();
    assert_eq!(status_of(&tmp, &id), RunStatus::Done);
    assert!(instance_destroyed(&tmp, &id));
    assert!(!open(&tmp)
        .list_events(&id)
        .unwrap()
        .iter()
        .any(|e| e.stage == "stage_failed"));
}

#[test]
fn fix_status_dry_run_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let id = seed(&tmp, DEAD_PID, 0, false);
    let out = command(&tmp)
        .args(["fix-status", "--dry-run", &id.to_string()])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&out.stdout).contains("marking failed"));
    assert_eq!(status_of(&tmp, &id), RunStatus::Running);
    assert!(!instance_destroyed(&tmp, &id));
    assert!(open(&tmp).list_events(&id).unwrap().is_empty());
}

#[test]
fn fix_status_leaves_a_live_local_run_alone() {
    let tmp = TempDir::new().unwrap();
    // The test process itself is alive and started before the pid file was
    // written (fresh mtime) — the genuine-process case.
    let id = seed(&tmp, std::process::id(), 0, false);
    let out = command(&tmp)
        .args(["fix-status", &id.to_string()])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&out.stdout).contains("still alive"));
    assert_eq!(status_of(&tmp, &id), RunStatus::Running);
}

#[test]
fn fix_status_treats_a_recycled_pid_as_dead() {
    if !start_time_readable() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    // run.pid names a live process (this one), but the file was written ten
    // days before that process started: the PID was recycled after a reboot.
    let id = seed(&tmp, std::process::id(), 10 * 86_400, false);
    command(&tmp)
        .args(["fix-status", &id.to_string()])
        .assert()
        .success();
    assert_eq!(status_of(&tmp, &id), RunStatus::Failed);
}

#[test]
fn resume_reconciles_a_dead_local_run_without_spawning_a_poller() {
    let tmp = TempDir::new().unwrap();
    let id = seed(&tmp, DEAD_PID, 0, false);
    let out = command(&tmp)
        .args(["resume", &id.to_string(), "--json"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let report = &v["runs"][0];
    assert_eq!(report["outcome"], "reconciled");
    assert_eq!(report["final_status"], "failed");
    assert!(report.get("poller_pid").is_none());
    assert_eq!(status_of(&tmp, &id), RunStatus::Failed);
    assert!(instance_destroyed(&tmp, &id));
    // spawn_daemon would have created poller.log in the run dir.
    assert!(!run_dir(&tmp, &id).join("poller.log").exists());
}

#[test]
fn resume_reports_done_for_a_dead_local_run_that_finished_ok() {
    let tmp = TempDir::new().unwrap();
    let id = seed(&tmp, DEAD_PID, 0, true);
    let out = command(&tmp)
        .args(["resume", &id.to_string(), "--json"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["runs"][0]["outcome"], "reconciled");
    assert_eq!(v["runs"][0]["final_status"], "done");
    assert_eq!(status_of(&tmp, &id), RunStatus::Done);
}
