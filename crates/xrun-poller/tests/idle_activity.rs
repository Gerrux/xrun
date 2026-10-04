//! What counts as activity for the idle cap, and the local stdout snapshot.

use std::cell::Cell;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{Duration, Utc};
use tempfile::TempDir;
use xrun_core::{
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{InstanceCaps, RunId, RunStatus, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};
use xrun_poller::{CancellationToken, Poller, PollerConfig};

/// Reads metrics/stdout from real files (like the local adapter); the events
/// file yields a `done` line on the `done_on`-th tail.
struct FileVendor {
    done_on: usize,
    event_tails: Cell<usize>,
    destroys: Arc<AtomicUsize>,
}

impl VendorAdapter for FileVendor {
    fn name(&self) -> &'static str {
        "file"
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
    fn tail(&self, _h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError> {
        if file.contains("events") {
            let n = self.event_tails.get() + 1;
            self.event_tails.set(n);
            if n == self.done_on {
                let ts = Utc::now().to_rfc3339();
                return Ok(
                    format!("{{\"ts\":\"{ts}\",\"stage\":\"done\",\"status\":\"ok\"}}\n")
                        .into_bytes(),
                );
            }
            return Ok(Vec::new());
        }
        let Ok(mut f) = std::fs::File::open(file) else {
            return Ok(Vec::new());
        };
        let len = f.metadata().unwrap().len();
        if offset >= len {
            return Ok(Vec::new());
        }
        f.seek(SeekFrom::Start(offset)).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        Ok(buf)
    }
    fn pull(&self, _: &InstanceHandle, _: &str, _: &Path) -> Result<(), VendorError> {
        Ok(())
    }
    fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
        self.destroys.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn handle() -> InstanceHandle {
    InstanceHandle {
        id: "inst-1".to_string(),
        vendor: "file".to_string(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "root".to_string(),
    }
}

fn setup(tmp: &TempDir, idle_cap_secs: Option<i64>) -> RunId {
    let mut store = Store::open(&tmp.path().join("runs.db")).unwrap();
    let run_id = store
        .create_run("r", "hash", "manifest.yaml", "local", &[])
        .unwrap();
    store
        .update_run_status(&run_id, RunStatus::Running)
        .unwrap();
    store
        .update_run_started_at(&run_id, Utc::now() - Duration::hours(1))
        .unwrap();
    store
        .insert_instance_with_caps(
            "inst-1",
            "local",
            Some(&run_id),
            None,
            None,
            Utc::now() - Duration::hours(1),
            &InstanceCaps {
                idle_timeout_secs: idle_cap_secs,
                ..Default::default()
            },
        )
        .unwrap();
    std::fs::create_dir_all(tmp.path().join("runs").join(run_id.to_string())).unwrap();
    run_id
}

fn run(tmp: &TempDir, run_id: &RunId, stdout_file: &Path, done_on: usize) -> (RunStatus, usize) {
    let destroys = Arc::new(AtomicUsize::new(0));
    let vendor = FileVendor {
        done_on,
        event_tails: Cell::new(0),
        destroys: destroys.clone(),
    };
    let status = Poller::new(
        run_id.clone(),
        Store::open(&tmp.path().join("runs.db")).unwrap(),
        Box::new(vendor),
        handle(),
        tmp.path().join("runs"),
    )
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        events_file: "events.jsonl".into(),
        metrics_file: tmp.path().join("no-metrics.jsonl").display().to_string(),
        stdout_file: stdout_file.display().to_string(),
        ..Default::default()
    })
    .run(CancellationToken::new())
    .unwrap();
    (status, destroys.load(Ordering::SeqCst))
}

/// Local runs tail `<runs>/<id>/stdout.log`, the very file the poller
/// snapshots stdout into: appending what it just read made the file grow
/// every tick (duplicated log, re-parsed metric lines, endless "activity").
#[test]
fn local_stdout_snapshot_does_not_feed_back_into_itself() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp, None);
    let log: PathBuf = tmp
        .path()
        .join("runs")
        .join(run_id.to_string())
        .join("stdout.log");
    std::fs::write(&log, "epoch 1 batch 7/900\n").unwrap();
    let (status, _) = run(&tmp, &run_id, &log, 5);
    assert_eq!(status, RunStatus::Done);
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "epoch 1 batch 7/900\n"
    );
}

/// Plain stdout (no `key=value` metric in it) is output, so it resets the idle
/// timer — "no output for N minutes" is what `on_idle_minutes` promises.
#[test]
fn plain_stdout_counts_as_activity_for_the_idle_cap() {
    let tmp = TempDir::new().unwrap();
    // Anchor would be created_at (1 h ago) without activity: 5 min cap fires.
    let run_id = setup(&tmp, Some(300));
    let remote = tmp.path().join("remote-stdout.log");
    std::fs::write(&remote, "epoch 1 batch 7/900\n").unwrap();
    let (status, _) = run(&tmp, &run_id, &remote, 3);
    assert_eq!(status, RunStatus::Done, "a printing run is not idle");
    let inst = Store::open(&tmp.path().join("runs.db"))
        .unwrap()
        .get_instance("inst-1")
        .unwrap()
        .unwrap();
    assert!(inst.auto_destroyed_reason.is_none());
    assert!(inst.last_active_at.is_some());
    // Snapshot still lands in the run dir for `xrun logs`.
    let snap = tmp
        .path()
        .join("runs")
        .join(run_id.to_string())
        .join("stdout.log");
    assert_eq!(
        std::fs::read_to_string(snap).unwrap(),
        "epoch 1 batch 7/900\n"
    );
}

/// Control: same setup with no output at all still trips the idle cap.
#[test]
fn silent_run_still_trips_the_idle_cap() {
    let tmp = TempDir::new().unwrap();
    let run_id = setup(&tmp, Some(300));
    let (status, destroys) = run(&tmp, &run_id, &tmp.path().join("absent.log"), 3);
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(destroys, 1);
}
