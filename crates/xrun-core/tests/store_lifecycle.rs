use chrono::Utc;
use xrun_core::store::{ListFilter, NewArtifact, NewEvent, NewMetric, RunStatus, Store};

#[test]
fn lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("runs.db");

    let mut store = Store::open(&db_path).unwrap();

    let run_id = store
        .create_run("my-run", "abc123hash", "runs/1/manifest.yaml", "vast", &[])
        .unwrap();

    // Three events
    for i in 0u32..3 {
        store
            .append_event(
                &run_id,
                NewEvent {
                    ts: Utc::now(),
                    stage: format!("stage{i}"),
                    status: "start".to_string(),
                    msg: Some(format!("message {i}")),
                    payload_json: None,
                },
            )
            .unwrap();
    }

    // Five metrics across two keys
    for step in 1i64..=3 {
        store
            .append_metric(
                &run_id,
                NewMetric {
                    step,
                    key: "loss".to_string(),
                    value: 1.0 / step as f64,
                    ts: Utc::now(),
                },
            )
            .unwrap();
    }
    for step in 1i64..=2 {
        store
            .append_metric(
                &run_id,
                NewMetric {
                    step,
                    key: "acc".to_string(),
                    value: step as f64 * 0.1,
                    ts: Utc::now(),
                },
            )
            .unwrap();
    }

    // List runs by provisioning status
    let runs = store
        .list_runs(&ListFilter {
            status: Some(RunStatus::Provisioning),
            vendor: None,
        })
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, run_id);
    assert_eq!(runs[0].name, "my-run");

    // Empty result for a different status
    let running = store
        .list_runs(&ListFilter {
            status: Some(RunStatus::Running),
            vendor: None,
        })
        .unwrap();
    assert!(running.is_empty());

    // Poll offset: update twice, read back the second value
    store
        .update_poll_offset(&run_id, "events.jsonl", 100)
        .unwrap();
    store
        .update_poll_offset(&run_id, "events.jsonl", 250)
        .unwrap();
    let offset = store.get_poll_offset(&run_id, "events.jsonl").unwrap();
    assert_eq!(offset, 250);

    // Default offset for an unset file
    let zero = store.get_poll_offset(&run_id, "metrics.jsonl").unwrap();
    assert_eq!(zero, 0);
}

#[test]
fn record_artifact_and_update_status() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("runs.db");
    let mut store = Store::open(&db_path).unwrap();

    let run_id = store
        .create_run("art-run", "hash2", "runs/2/manifest.yaml", "vast", &[])
        .unwrap();

    store
        .record_artifact(
            &run_id,
            NewArtifact {
                kind: "checkpoint".to_string(),
                remote_path: "/workspace/checkpoints/epoch10.pt".to_string(),
                local_path: None,
                size_bytes: Some(1024 * 1024),
                sha256: None,
                is_best: true,
            },
        )
        .unwrap();

    store.update_run_status(&run_id, RunStatus::Done).unwrap();

    let run = store.get_run(&run_id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Done);
}

#[test]
fn reopen_existing_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("runs.db");

    let run_id = {
        let mut store = Store::open(&db_path).unwrap();
        store
            .create_run("persist-run", "hashXYZ", "runs/3/manifest.yaml", "kaggle", &[])
            .unwrap()
    };

    // Reopen — schema_version must remain 1, data must persist
    let store = Store::open(&db_path).unwrap();
    let run = store.get_run(&run_id).unwrap();
    assert!(run.is_some());
    let run = run.unwrap();
    assert_eq!(run.name, "persist-run");
    assert_eq!(run.vendor, "kaggle");
    assert_eq!(run.status, RunStatus::Provisioning);
}

#[test]
fn update_nonexistent_run_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("runs.db");
    let mut store = Store::open(&db_path).unwrap();

    // A freshly generated ID that was never inserted
    let fake_id = xrun_core::RunId::new();
    let result = store.update_run_status(&fake_id, RunStatus::Failed);
    assert!(result.is_err());
}
