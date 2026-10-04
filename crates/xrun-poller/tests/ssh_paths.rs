//! An ssh-style run: the training writes events/metrics/stdout under
//! `<workdir_root>/<run_id>/`, not `/workspace/run`. With a `PollerConfig`
//! pointing there (what `xrun launch` / the poll-daemon build for ssh) the
//! poller sees `done` and finishes; the mock serves nothing at other paths.

use std::cell::RefCell;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use tempfile::TempDir;
use xrun_core::{
    error::VendorError,
    manifest::{DataSource, DonePolicy, Manifest, RunSpec},
    store::{RunStatus, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};
use xrun_poller::{CancellationToken, Poller, PollerConfig};

const DIR: &str = "/data/xrun/RUN1";

struct SshLikeVendor {
    asked: Arc<Mutex<Vec<String>>>,
    destroys: Arc<Mutex<u32>>,
    served: RefCell<bool>,
    /// `Some(false)`: PID gone (after train_start). `None`: probe unknown.
    alive: Option<bool>,
    events: Vec<u8>,
}

impl VendorAdapter for SshLikeVendor {
    fn name(&self) -> &'static str {
        "ssh"
    }
    fn validate(&self, _: &Manifest) -> Result<(), VendorError> {
        Ok(())
    }
    fn dry_run_plan(&self, _: &Manifest) -> Result<DryRunPlan, VendorError> {
        Err(VendorError::NotImplemented)
    }
    fn provision(&self, _: &Manifest) -> Result<InstanceHandle, VendorError> {
        Err(VendorError::NotImplemented)
    }
    fn upload(&self, _: &InstanceHandle, _: &[DataSource]) -> Result<(), VendorError> {
        Ok(())
    }
    fn execute(&self, _: &InstanceHandle, _: &RunSpec) -> Result<(), VendorError> {
        Ok(())
    }
    fn tail(&self, _: &InstanceHandle, file: &str, _: u64) -> Result<Vec<u8>, VendorError> {
        self.asked.lock().unwrap().push(file.to_string());
        if file == format!("{DIR}/events.jsonl") && !*self.served.borrow() {
            *self.served.borrow_mut() = true;
            return Ok(self.events.clone());
        }
        Ok(Vec::new())
    }
    fn pull(&self, _: &InstanceHandle, _: &str, _: &Path) -> Result<(), VendorError> {
        Ok(())
    }
    fn process_alive(&self, _: &InstanceHandle) -> Option<bool> {
        self.alive
    }
    fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
        *self.destroys.lock().unwrap() += 1;
        Ok(())
    }
}

fn ssh_config() -> PollerConfig {
    PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        events_file: format!("{DIR}/events.jsonl"),
        metrics_file: format!("{DIR}/metrics.jsonl"),
        stdout_file: format!("{DIR}/stdout.log"),
        ..Default::default()
    }
}

struct Outcome {
    status: RunStatus,
    files: Vec<String>,
    destroys: u32,
    instance_destroyed: bool,
}

fn run_with(events: String, alive: Option<bool>, with_train_start: bool) -> Outcome {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("runs.db");
    let mut store = Store::open(&db).unwrap();
    let run_id = store
        .create_run("ssh_run", "hash", "manifest.yaml", "ssh", &[])
        .unwrap();
    store
        .insert_instance("ssh-ws-1", "ssh", Some(&run_id), None, None, Utc::now())
        .unwrap();
    store
        .update_run_status(&run_id, RunStatus::Running)
        .unwrap();
    store
        .update_run_started_at(&run_id, Utc::now() - Duration::minutes(5))
        .unwrap();
    if with_train_start {
        // What SshAdapter::execute records; the poller hydrates from it.
        store
            .append_event(
                &run_id,
                xrun_core::store::NewEvent {
                    ts: Utc::now(),
                    stage: "train_start".into(),
                    status: "ok".into(),
                    msg: None,
                    payload_json: None,
                },
            )
            .unwrap();
    }
    let asked = Arc::new(Mutex::new(Vec::new()));
    let destroys = Arc::new(Mutex::new(0));
    let vendor = SshLikeVendor {
        asked: asked.clone(),
        destroys: destroys.clone(),
        served: RefCell::new(false),
        alive,
        events: events.into_bytes(),
    };
    let handle = InstanceHandle {
        id: "ssh-ws-1".into(),
        vendor: "ssh".into(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "u".into(),
    };
    let status = Poller::new(
        run_id,
        store,
        Box::new(vendor),
        handle,
        tmp.path().join("runs"),
    )
    .with_config(ssh_config())
    // What `xrun launch` wires for an ssh manifest (kill_remote = false).
    .with_done_policy(DonePolicy::from_manifest(
        &Manifest::from_yaml_str("name: s\nvendor: ssh\nssh:\n  host_alias: ws\nrun:\n  cmd: t\n")
            .unwrap(),
    ))
    .run(CancellationToken::new())
    .unwrap();
    let files = asked.lock().unwrap().clone();
    let destroys = *destroys.lock().unwrap();
    let instance_destroyed = Store::open(&db)
        .unwrap()
        .get_instance("ssh-ws-1")
        .unwrap()
        .unwrap()
        .destroyed_at
        .is_some();
    Outcome {
        status,
        files,
        destroys,
        instance_destroyed,
    }
}

#[test]
fn done_event_at_the_ssh_run_dir_finishes_the_run() {
    let ts = Utc::now().to_rfc3339();
    let ev = format!("{{\"ts\":\"{ts}\",\"stage\":\"done\",\"status\":\"ok\"}}\n");
    let o = run_with(ev, Some(true), true);
    assert_eq!(o.status, RunStatus::Done);
    let files = &o.files;
    assert!(files.iter().all(|f| f.starts_with(DIR)), "{files:?}");
    assert!(files.contains(&format!("{DIR}/events.jsonl")));
    assert!(files.contains(&format!("{DIR}/stdout.log")));
    // on_done on ssh: marked destroyed, the (finished) PID is never signalled.
    assert_eq!(o.destroys, 0);
    assert!(o.instance_destroyed);
}

#[test]
fn vanished_pid_after_train_start_fails_the_ssh_run_without_signalling_it() {
    // No done:ok; the remote PID is gone -> Failed (not a hang). The process
    // is already dead, so `destroy` (kill of run.pid, maybe recycled by now)
    // must not run; the instance is only marked destroyed.
    let o = run_with(String::new(), Some(false), true);
    assert_eq!(o.status, RunStatus::Failed);
    assert_eq!(o.destroys, 0);
    assert!(o.instance_destroyed);
}

#[test]
fn fail_event_on_ssh_stops_the_still_running_process() {
    // A stage `fail` while the PID is alive: on_stage_failed=stop_instance
    // (default) kills the remote process via destroy.
    let ts = Utc::now().to_rfc3339();
    let ev = format!("{{\"ts\":\"{ts}\",\"stage\":\"train\",\"status\":\"fail\"}}\n");
    let o = run_with(ev, Some(true), true);
    assert_eq!(o.status, RunStatus::Failed);
    assert_eq!(o.destroys, 1);
}
