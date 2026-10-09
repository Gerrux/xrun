use base64::Engine;
use xrun_core::manifest::Manifest;

use super::*;
use crate::bridge::{FakeBridge, RunOut};

fn manifest(extra: &str) -> Manifest {
    let y = format!(
        "name: my_exp\nvendor: lightning\nlightning:\n  machine: L4\n  gpu: cpu\n{extra}\nrun:\n  cmd: python train.py\n  setup: pip install -r req.txt\n  args:\n    --lr: '5e-4'\n"
    );
    Manifest::from_yaml_str(&y).expect("parse")
}

fn store() -> (tempfile::TempDir, Store) {
    let td = tempfile::TempDir::new().unwrap();
    let s = Store::open(&td.path().join("runs.db")).unwrap();
    (td, s)
}

fn out(output: &str, exit_code: i32) -> RunOut {
    RunOut {
        output: output.to_string(),
        exit_code,
    }
}

fn adapter(fake: &FakeBridge) -> (tempfile::TempDir, LightningAdapter, RunId) {
    let (td, mut s) = store();
    let rid = s
        .create_run("my_exp", "hash", "exp/x.yaml", "lightning", &[])
        .unwrap();
    let a = LightningAdapter::with_bridge(s, None, Box::new(fake.clone()));
    a.set_run_id(&rid);
    (td, a, rid)
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[test]
fn validate_rejects_other_vendor_missing_cmd_and_absolute_dst() {
    let fake = FakeBridge::new();
    let (_td, a, _) = adapter(&fake);
    a.validate(&manifest("")).expect("valid");

    let ssh = Manifest::from_yaml_str("name: x\nvendor: local\nrun:\n  cmd: echo\n").unwrap();
    assert!(matches!(a.validate(&ssh), Err(VendorError::Validation(_))));

    let no_cmd =
        Manifest::from_yaml_str("name: x\nvendor: lightning\nrun:\n  setup: ls\n").unwrap();
    assert!(a.validate(&no_cmd).is_err());

    // Core validation already rejects an absolute dst at parse time; the
    // adapter repeats the check for manifests built in code.
    let mut abs = manifest("data:\n  - src: a.txt\n    dst: rel/a.txt");
    abs.data.as_mut().unwrap()[0].dst = "/abs/a.txt".to_string();
    let err = a.validate(&abs).expect_err("absolute dst");
    assert!(err.to_string().contains("relative"), "{err}");
}

#[test]
fn dry_run_plan_reports_machine_and_cmd() {
    let fake = FakeBridge::new();
    let (_td, a, _) = adapter(&fake);
    let plan = a.dry_run_plan(&manifest("")).unwrap();
    assert_eq!(plan.gpu_query, "L4");
    assert_eq!(plan.estimated_price_max, 0.0);
    assert!(plan.cmd_line.contains("python train.py --lr 5e-4"));
}

#[test]
fn full_lifecycle() {
    let fake = FakeBridge::new();
    fake.set_run_fn(|c| {
        if c.contains("tail -c") {
            // 11 bytes of log; the fake ignores the offset.
            out(&format!("11\n{}\n", b64("hello world")), 0)
        } else if c.contains("echo $$") {
            out("4242\n", 0)
        } else if c.contains("globstar") {
            out(
                "xrun/RID/checkpoints/best.pt\nxrun/RID/checkpoints/last.pt\n",
                0,
            )
        } else {
            out("", 0)
        }
    });
    let (td, a, rid) = adapter(&fake);
    let m = manifest("");

    // provision
    let h = a.provision(&m).expect("provision");
    assert_eq!(h.vendor, "lightning");
    assert_eq!(h.id, format!("lightning-xrun-my-exp-{rid}"));
    assert_eq!(h.ssh_host.as_deref(), Some("xrun-my-exp"));
    assert_eq!(h.ssh_user, "tester/default");
    assert_eq!(h.ssh_port, None);
    assert_eq!(h.run_dir.as_deref(), Some(format!("xrun/{rid}").as_str()));
    assert!(fake
        .calls()
        .iter()
        .any(|c| c.starts_with("studio_start xrun-my-exp machine=L4 interruptible=true")));

    // upload: a file (mkdir parent first) and a directory
    let data = td.path().join("train.h5");
    std::fs::write(&data, b"x").unwrap();
    let dir = td.path().join("imgs");
    std::fs::create_dir(&dir).unwrap();
    a.upload(
        &h,
        &[
            DataSource {
                src: data.display().to_string(),
                dst: "~/data/train.h5".into(),
                mode: None,
                unpack: None,
                exclude: vec![],
                compress: None,
            },
            DataSource {
                src: dir.display().to_string(),
                dst: "imgs".into(),
                mode: None,
                unpack: None,
                exclude: vec![],
                compress: None,
            },
        ],
    )
    .expect("upload");
    {
        let st = fake.state.lock().unwrap();
        assert!(st.uploaded.iter().any(|(_, r)| r == "data/train.h5"));
        assert!(st.uploaded.iter().any(|(_, r)| r == "imgs"));
        assert!(st.runs.iter().any(|c| c == "mkdir -p \"$HOME\"/'data'"));
    }

    // execute: setup then detached launch
    a.execute(&h, &m.run).expect("execute");
    {
        let st = fake.state.lock().unwrap();
        let setup = st.runs.iter().find(|c| c.contains("pip install")).unwrap();
        assert!(setup.contains("cd "), "{setup}");
        let launch = st.runs.iter().find(|c| c.contains("echo $$")).unwrap();
        assert!(launch.contains("python train.py --lr 5e-4"), "{launch}");
        assert!(launch.contains("CUDA_VISIBLE_DEVICES="), "{launch}");
        assert!(launch.contains("XRUN_RUN_DIR="), "{launch}");
    }

    // tail
    assert_eq!(
        a.tail(&h, &format!("xrun/{rid}/stdout.log"), 0).unwrap(),
        b"hello world"
    );
    assert!(a.tail(&h, "xrun/x/f", 11).unwrap().is_empty()); // size == offset
    assert!(matches!(
        a.tail(&h, "xrun/x/f", 50),
        Err(VendorError::Truncated)
    )); // size < offset

    // process_alive via exit code
    assert_eq!(a.process_alive(&h), Some(true));
    fake.set_run_fn(|_| out("", 1));
    assert_eq!(a.process_alive(&h), Some(false));
    fake.set_run_fn(|_| out("", 2));
    assert_eq!(a.process_alive(&h), None);

    // pull
    fake.set_run_fn(|c| {
        if c.contains("globstar") {
            out(
                "xrun/RID/checkpoints/best.pt\nxrun/RID/checkpoints/last.pt\n",
                0,
            )
        } else {
            out("", 0)
        }
    });
    {
        let mut st = fake.state.lock().unwrap();
        st.remote_files
            .insert("xrun/RID/checkpoints/best.pt".into(), b"BEST".to_vec());
        st.remote_files
            .insert("xrun/RID/checkpoints/last.pt".into(), b"LAST".to_vec());
    }
    let into = td.path().join("models");
    a.pull(&h, "checkpoints/*.pt", &into).expect("pull");
    assert_eq!(std::fs::read(into.join("best.pt")).unwrap(), b"BEST");
    assert_eq!(std::fs::read(into.join("last.pt")).unwrap(), b"LAST");
    {
        let st = fake.state.lock().unwrap();
        let script = st.runs.iter().find(|c| c.contains("globstar")).unwrap();
        assert!(
            script.contains(&format!("xrun/{rid}/checkpoints/")),
            "{script}"
        );
    }
    let conn = rusqlite::Connection::open(td.path().join("runs.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT kind, sha256 FROM artifacts WHERE run_id = ?1")
        .unwrap();
    let arts: Vec<(String, Option<String>)> = stmt
        .query_map([rid.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(arts.len(), 2);
    assert!(arts.iter().all(|x| x.0 == "checkpoint"));
    assert!(arts
        .iter()
        .all(|x| x.1.as_deref().map(str::len) == Some(64)));

    // vendor_instances sees the active studio
    let inst = a.vendor_instances().unwrap();
    assert_eq!(inst.len(), 1);
    assert_eq!(inst[0].status.as_deref(), Some("running"));
    assert_eq!(inst[0].gpu.as_deref(), Some("L4"));

    // destroy: kill, stop studio, mark destroyed
    fake.set_run_fn(|_| out("", 0));
    a.destroy(&h).expect("destroy");
    let calls = fake.calls();
    assert!(
        calls.iter().any(|c| c == "studio_stop xrun-my-exp"),
        "{calls:?}"
    );
    let kill_pos = calls.iter().position(|c| c.contains("kill -TERM")).unwrap();
    let stop_pos = calls
        .iter()
        .position(|c| c.starts_with("studio_stop"))
        .unwrap();
    assert!(kill_pos < stop_pos);
    assert!(a.vendor_instances().unwrap().is_empty(), "marked destroyed");

    // events recorded
    let events = a
        .store
        .borrow()
        .as_ref()
        .unwrap()
        .list_events(&rid)
        .unwrap();
    let stages: Vec<_> = events.iter().map(|e| e.stage.as_str()).collect();
    for s in [
        "provision",
        "upload",
        "env_ready",
        "train_start",
        "pull",
        "instance_destroyed",
    ] {
        assert!(stages.contains(&s), "missing {s}: {stages:?}");
    }
}

#[test]
fn destroy_stops_studio_even_if_kill_fails() {
    let fake = FakeBridge::new();
    let (_td, a, _rid) = adapter(&fake);
    let h = a.provision(&manifest("")).unwrap();
    fake.set_run_fn(|_| out("boom", 1));
    a.destroy(&h).expect("still destroys");
    assert!(fake.calls().iter().any(|c| c.starts_with("studio_stop")));
}

#[test]
fn provision_failure_is_reported_and_no_instance_row() {
    let fake = FakeBridge::new();
    fake.state.lock().unwrap().fail_start = true;
    let (_td, a, _) = adapter(&fake);
    assert!(a.provision(&manifest("")).is_err());
    assert!(a.vendor_instances().unwrap().is_empty());
}

#[test]
fn manifest_teamspace_and_workdir_apply() {
    let fake = FakeBridge::new();
    let (_td, a, rid) = adapter(&fake);
    let h = a
        .provision(&manifest(
            "  teamspace: me/proj\n  workdir: runs/x\n  max_runtime_secs: 600",
        ))
        .unwrap();
    assert_eq!(h.ssh_user, "me/proj");
    assert_eq!(h.run_dir.as_deref(), Some(format!("runs/x/{rid}").as_str()));
    assert!(fake
        .calls()
        .iter()
        .any(|c| c.contains("max_runtime=Some(600)")));
}

#[test]
fn vendor_status_reports_account() {
    let fake = FakeBridge::new();
    let (_td, a, _) = adapter(&fake);
    let s = a.vendor_status().unwrap();
    assert!(s.connected);
    assert_eq!(s.account.as_deref(), Some("tester · tester/default"));
    assert_eq!(s.balance, None);
}

#[test]
fn process_alive_false_when_studio_not_running_none_when_status_errors() {
    let fake = FakeBridge::new();
    let (_td, a, _rid) = adapter(&fake);
    let h = a.provision(&manifest("")).unwrap();
    fake.state.lock().unwrap().status = "stopped".into();
    assert_eq!(a.process_alive(&h), Some(false));
    // Status unavailable (bad handle -> no studio ref) stays unknown.
    let mut bad = h.clone();
    bad.ssh_host = None;
    assert_eq!(a.process_alive(&bad), None);
}

#[test]
fn launch_and_kill_scripts_are_not_replayable() {
    let fake = FakeBridge::new();
    let (_td, a, _rid) = adapter(&fake);
    let m = manifest("");
    a.validate(&m).unwrap();
    let h = a.provision(&m).unwrap();
    a.execute(&h, &m.run).unwrap();
    a.destroy(&h).unwrap();
    let st = fake.state.lock().unwrap();
    assert!(st.once_runs.iter().any(|c| c.contains("echo $$")), "launch");
    assert!(
        st.once_runs.iter().any(|c| c.contains("kill -TERM")),
        "kill"
    );
    // Only the launch and the kill script; setup / mkdir probes stay replayable.
    assert_eq!(st.once_runs.len(), 2, "{:?}", st.once_runs);
}

#[test]
fn reused_stopped_studio_is_restarted_before_upload_and_execute() {
    let td2 = tempfile::TempDir::new().unwrap();
    let src = td2.path().join("a.txt");
    std::fs::write(&src, b"x").unwrap();
    let ds = DataSource {
        src: src.display().to_string(),
        dst: "a.txt".into(),
        mode: None,
        unpack: None,
        exclude: vec![],
        compress: None,
    };
    let starts = |f: &FakeBridge| {
        f.calls()
            .iter()
            .filter(|c| c.starts_with("studio_start"))
            .count()
    };

    let fake = FakeBridge::new();
    let (_td, a, _rid) = adapter(&fake);
    let m = manifest("");
    let h = a.provision(&m).unwrap();
    assert_eq!(starts(&fake), 1);

    // Running: no extra start.
    a.validate(&m).unwrap();
    a.upload(&h, std::slice::from_ref(&ds)).unwrap();
    a.execute(&h, &m.run).unwrap();
    assert_eq!(starts(&fake), 1);

    // Stopped (reuse path: provision skipped): started once with the stashed params.
    fake.state.lock().unwrap().status = "stopped".into();
    a.upload(&h, std::slice::from_ref(&ds)).unwrap();
    assert_eq!(starts(&fake), 2);
    let last = fake
        .calls()
        .into_iter()
        .rfind(|c| c.starts_with("studio_start"))
        .unwrap();
    assert!(last.contains("machine=L4 interruptible=true"), "{last}");
}
