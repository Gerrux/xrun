use xrun_core::{
    manifest::Manifest,
    store::{RunId, Store},
    vendor::{InstanceHandle, VendorAdapter},
};

use crate::bridge::{ExecOutput, FakeBridge, FakeCall, SessionInfo};
use crate::cmd;
use crate::ColabAdapter;

struct Env {
    _td: tempfile::TempDir,
    adapter: ColabAdapter,
    fake: FakeBridge,
    run_id: RunId,
}

fn env() -> Env {
    let td = tempfile::TempDir::new().unwrap();
    let mut store = Store::open(&td.path().join("runs.db")).unwrap();
    let run_id = store.create_run("t", "h", "m.yaml", "colab", &[]).unwrap();
    let fake = FakeBridge::new();
    let adapter = ColabAdapter::with_bridge(store, Box::new(fake.clone()));
    adapter.set_run_id(&run_id);
    Env {
        _td: td,
        adapter,
        fake,
        run_id,
    }
}

fn manifest(extra: &str, data: &str) -> Manifest {
    let yaml = format!(
        "name: colab-test\nvendor: colab\n{extra}\n{data}\nrun:\n  cmd: python train.py\n  setup: pip install x\n  args:\n    --lr: 5e-4\n"
    );
    Manifest::from_yaml_str(&yaml).expect("parse")
}

fn plain() -> Manifest {
    manifest("", "")
}

fn count_calls(fake: &FakeBridge, f: impl Fn(&FakeCall) -> bool) -> usize {
    fake.calls().iter().filter(|c| f(c)).count()
}

#[test]
fn lifecycle_provision_to_destroy() {
    let e = env();
    let a = &e.adapter;
    let rid = e.run_id.to_string();
    let session = format!("xrun-{}", rid.to_lowercase());

    // data: one file, one dir
    let data = tempfile::TempDir::new().unwrap();
    std::fs::write(data.path().join("a.csv"), "1,2").unwrap();
    std::fs::create_dir_all(data.path().join("d/sub")).unwrap();
    std::fs::write(data.path().join("d/x.txt"), "x").unwrap();
    std::fs::write(data.path().join("d/sub/y.txt"), "y").unwrap();
    let p = |s: &str| data.path().join(s).to_string_lossy().replace('\\', "/");
    let m = manifest(
        "colab:\n  gpu: l4\n  high_mem: true",
        &format!(
            "data:\n  - src: \"{}\"\n    dst: /content/data/a.csv\n  - src: \"{}\"\n    dst: /content/data/d\n",
            p("a.csv"),
            p("d")
        ),
    );

    // provision
    let h = a.provision(&m).unwrap();
    assert_eq!(h.id, format!("colab-{session}"));
    assert_eq!(h.vendor, "colab");
    assert_eq!(h.ssh_user, "root");
    assert_eq!(
        h.ssh_host.as_deref(),
        Some(format!("gpu-{session}-ep").as_str())
    );
    let run_dir = format!("/content/xrun/{rid}");
    assert_eq!(h.run_dir.as_deref(), Some(run_dir.as_str()));
    assert!(e.fake.calls().contains(&FakeCall::SessionNew {
        name: session.clone(),
        gpu: "L4".into(),
        high_mem: true
    }));
    assert!(e.fake.exec_codes()[0].contains(&format!("mkdir -p '{run_dir}'")));

    // upload
    a.upload(&h, m.data.as_ref().unwrap()).unwrap();
    let uploads: Vec<String> = e
        .fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            FakeCall::Upload { remote, .. } => Some(remote),
            _ => None,
        })
        .collect();
    assert_eq!(
        uploads,
        vec![
            "/content/data/a.csv",
            "/content/data/d/sub/y.txt",
            "/content/data/d/x.txt"
        ]
    );
    let all = e.fake.exec_codes().join("\n");
    assert!(all.contains("mkdir -p '/content/data'"), "{all}");
    assert!(
        all.contains("'/content/data/d' '/content/data/d/sub'"),
        "{all}"
    );

    // execute: setup (sh) then spawn
    let before = e.fake.exec_codes().len();
    e.fake.script_exec(
        "cat '/content/xrun",
        ExecOutput::stdout(cmd::encode_sh(0, "5151\n", "")),
    );
    a.execute(&h, &m.run).unwrap();
    let codes = e.fake.exec_codes();
    let new = &codes[before..];
    assert!(new[0].contains("mkdir -p"), "workdir mkdir");
    assert!(new[1].contains(cmd::MARK_SH) && new[1].contains("pip install x"));
    assert!(new[2].contains(cmd::MARK_SPAWN));
    assert!(new[2].contains("setsid nohup bash -c"), "{}", new[2]);
    assert!(new[2].contains("python train.py --lr 0.0005"));
    let setup_timeout = e
        .fake
        .calls()
        .into_iter()
        .find_map(|c| match c {
            FakeCall::Exec { code, timeout, .. } if code.contains("pip install x") => Some(timeout),
            _ => None,
        })
        .unwrap();
    assert_eq!(setup_timeout.as_secs(), 3600);

    // tail: data, empty, truncated
    let events = format!("{run_dir}/events.jsonl");
    e.fake.set_file(&events, b"hello world");
    assert_eq!(a.tail(&h, &events, 0).unwrap(), b"hello world");
    assert_eq!(a.tail(&h, &events, 6).unwrap(), b"world");
    assert!(a.tail(&h, &events, 11).unwrap().is_empty());
    assert!(matches!(
        a.tail(&h, &events, 99),
        Err(xrun_core::error::VendorError::Truncated)
    ));

    // process_alive
    assert_eq!(a.process_alive(&h), Some(true));
    e.fake.set_alive("dead");
    assert_eq!(a.process_alive(&h), Some(false));
    e.fake.set_alive("no_pid");
    assert_eq!(a.process_alive(&h), None);

    // pull
    let ck = format!("{run_dir}/checkpoints/best.pt");
    e.fake.set_glob(&[&ck]);
    e.fake.set_file(&ck, b"weights");
    let into = tempfile::TempDir::new().unwrap();
    a.pull(&h, "checkpoints/best*.pt", into.path()).unwrap();
    assert_eq!(
        std::fs::read(into.path().join("best.pt")).unwrap(),
        b"weights"
    );
    let globbed = e
        .fake
        .exec_codes()
        .into_iter()
        .rev()
        .find(|c| c.contains(cmd::MARK_GLOB))
        .unwrap();
    assert!(
        globbed.contains(&format!("{run_dir}/checkpoints/best*.pt")),
        "{globbed}"
    );

    // destroy: kill step first, then session_stop, instance marked destroyed
    let n_calls = e.fake.calls().len();
    a.destroy(&h).unwrap();
    let tail_calls = &e.fake.calls()[n_calls..];
    assert!(matches!(&tail_calls[0], FakeCall::Exec { code, .. } if code.contains("kill -TERM")));
    assert_eq!(
        tail_calls[1],
        FakeCall::SessionStop {
            name: session.clone()
        }
    );
    assert_eq!(
        count_calls(&e.fake, |c| matches!(c, FakeCall::SessionStop { .. })),
        1
    );
}

#[test]
fn artifacts_and_instance_row_are_recorded() {
    let e = env();
    let m = plain();
    let h = e.adapter.provision(&m).unwrap();
    let rd = h.run_dir.clone().unwrap();
    let f = format!("{rd}/out/best.pt");
    e.fake.set_glob(&[&f]);
    e.fake.set_file(&f, b"abc");
    let into = tempfile::TempDir::new().unwrap();
    e.adapter.pull(&h, "out/best.pt", into.path()).unwrap();
    e.adapter.destroy(&h).unwrap();

    let conn = rusqlite::Connection::open(e._td.path().join("runs.db")).unwrap();
    let rows: Vec<(String, String, Option<i64>, Option<String>)> = conn
        .prepare("SELECT kind, remote_path, size_bytes, sha256 FROM artifacts")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "checkpoint");
    assert_eq!(rows[0].1, f);
    assert_eq!(rows[0].2, Some(3));
    // sha256("abc")
    assert_eq!(
        rows[0].3.as_deref(),
        Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    let live: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM instances WHERE vendor='colab' AND destroyed_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live, 0, "instance marked destroyed");
}

#[test]
fn pull_with_no_match_is_an_error() {
    let e = env();
    let h = e.adapter.provision(&plain()).unwrap();
    e.fake.set_glob(&[]);
    let into = tempfile::TempDir::new().unwrap();
    assert!(e.adapter.pull(&h, "nope*", into.path()).is_err());
}

#[test]
fn pull_home_pattern_is_left_for_remote_expansion() {
    let e = env();
    let h = e.adapter.provision(&plain()).unwrap();
    e.fake.set_glob(&["/root/m.pt"]);
    let into = tempfile::TempDir::new().unwrap();
    e.adapter.pull(&h, "~/m.pt", into.path()).unwrap();
    let g = e
        .fake
        .exec_codes()
        .into_iter()
        .rev()
        .find(|c| c.contains(cmd::MARK_GLOB))
        .unwrap();
    assert!(g.contains("\"~/m.pt\""), "{g}");
}

#[test]
fn destroy_still_releases_session_when_kill_fails() {
    let e = env();
    let h = e.adapter.provision(&plain()).unwrap();
    e.fake.script_exec_err("kill -TERM", "session gone");
    e.adapter.destroy(&h).unwrap();
    assert_eq!(
        count_calls(&e.fake, |c| matches!(c, FakeCall::SessionStop { .. })),
        1
    );
}

#[test]
fn provision_releases_session_when_mkdir_fails() {
    let e = env();
    e.fake.script_exec(
        "mkdir -p",
        ExecOutput::stdout(cmd::encode_sh(1, "", "read-only")),
    );
    assert!(e.adapter.provision(&plain()).is_err());
    assert_eq!(
        count_calls(&e.fake, |c| matches!(c, FakeCall::SessionStop { .. })),
        1
    );
}

#[test]
fn provision_surfaces_session_new_failure() {
    let e = env();
    e.fake.fail_session_new("quota");
    let err = e.adapter.provision(&plain()).unwrap_err().to_string();
    assert!(err.contains("quota"), "{err}");
    assert_eq!(
        count_calls(&e.fake, |c| matches!(c, FakeCall::SessionStop { .. })),
        0
    );
}

#[test]
fn setup_failure_is_reported_and_nothing_is_spawned() {
    let e = env();
    let h = e.adapter.provision(&plain()).unwrap();
    e.fake.script_exec(
        "pip install x",
        ExecOutput::stdout(cmd::encode_sh(1, "", "no net")),
    );
    let err = e.adapter.execute(&h, &plain().run).unwrap_err().to_string();
    assert!(
        err.contains("setup failed") && err.contains("no net"),
        "{err}"
    );
    assert!(!e
        .fake
        .exec_codes()
        .iter()
        .any(|c| c.contains(cmd::MARK_SPAWN)));
}

#[test]
fn validate_rejects_bad_manifests() {
    let e = env();
    let a = &e.adapter;
    a.validate(&plain()).unwrap();

    let mut other = plain();
    other.vendor = xrun_core::manifest::Vendor::Ssh;
    assert!(a.validate(&other).is_err());

    let mut nocmd = plain();
    nocmd.run.cmd = None;
    let err = a.validate(&nocmd).unwrap_err().to_string();
    assert!(err.contains("run.cmd"), "{err}");

    // The manifest parser already rejects a relative dst; the adapter checks
    // it again for manifests built in code.
    let mut rel = manifest("", "data:\n  - src: a\n    dst: /abs/path\n");
    rel.data.as_mut().unwrap()[0].dst = "rel/path".into();
    let err = a.validate(&rel).unwrap_err().to_string();
    assert!(err.contains("dst"), "{err}");

    let bad_gpu =
        Manifest::from_yaml_str("name: x\nvendor: colab\ncolab:\n  gpu: V100\nrun:\n  cmd: echo\n");
    match bad_gpu {
        Err(_) => {}
        Ok(m) => assert!(a.validate(&m).is_err()),
    }
    let mut m = plain();
    m.colab = Some(xrun_core::manifest::ColabSpec {
        gpu: Some("V100".into()),
        ..Default::default()
    });
    assert!(a.validate(&m).is_err());
}

#[test]
fn dry_run_plan_is_free_and_reports_gpu() {
    let e = env();
    let plan = e
        .adapter
        .dry_run_plan(&manifest("colab:\n  gpu: a100", ""))
        .unwrap();
    assert_eq!(plan.gpu_query, "A100");
    assert_eq!(plan.estimated_price_max, 0.0);
    assert_eq!(plan.cmd_line, "python train.py --lr 0.0005");
    assert_eq!(e.adapter.dry_run_plan(&plain()).unwrap().gpu_query, "T4");
    assert_eq!(
        e.adapter
            .dry_run_plan(&manifest("colab:\n  gpu: CPU", ""))
            .unwrap()
            .gpu_query,
        "cpu"
    );
}

#[test]
fn vendor_status_reflects_login() {
    let e = env();
    let s = e.adapter.vendor_status().unwrap();
    assert!(s.connected);
    assert!(s.account.unwrap().contains("compute units"));
    e.fake.set_logged_in(false);
    let s = e.adapter.vendor_status().unwrap();
    assert!(!s.connected);
    assert!(s.error.unwrap().contains("login"));
}

#[test]
fn vendor_instances_joins_db_rows_with_listed_sessions() {
    let e = env();
    let h = e.adapter.provision(&plain()).unwrap();
    let rows = e.adapter.vendor_instances().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, h.id);
    assert_eq!(rows[0].status.as_deref(), Some("running"));
    assert_eq!(rows[0].gpu.as_deref(), Some("T4"));

    e.fake.set_sessions(vec![SessionInfo {
        name: "someone-else".into(),
        endpoint: "x".into(),
        accelerator: "T4".into(),
        variant: "GPU".into(),
    }]);
    let rows = e.adapter.vendor_instances().unwrap();
    assert_eq!(rows[0].status.as_deref(), Some("gone"));
}

#[test]
fn handle_without_run_id_still_finds_session_from_id() {
    let e = env();
    let h = InstanceHandle {
        id: "colab-xrun-abc".into(),
        vendor: "colab".into(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "root".into(),
        run_dir: None,
    };
    e.fake.set_file("/content/xrun/f", b"z");
    let _ = e.adapter.tail(&h, "/content/xrun/f", 0).unwrap();
    assert!(matches!(&e.fake.calls()[0], FakeCall::Exec { name, .. } if name == "xrun-abc"));
}

#[test]
fn spawn_setup_kill_and_session_ops_are_not_replayable() {
    let e = env();
    let h = e.adapter.provision(&plain()).unwrap();
    e.adapter.execute(&h, &plain().run).unwrap();
    e.adapter.destroy(&h).unwrap();
    let once = e.fake.exec_once_codes();
    assert!(once.iter().any(|c| c.contains(cmd::MARK_SPAWN)), "spawn");
    assert!(once.iter().any(|c| c.contains("pip install x")), "setup");
    assert!(once.iter().any(|c| c.contains("kill")), "kill");
    // Probes stay replayable; only spawn, setup and kill go through `exec_once`.
    assert_eq!(once.len(), 3, "{once:?}");
    assert!(!once.iter().any(|c| c.contains(cmd::MARK_TAIL)));
}
