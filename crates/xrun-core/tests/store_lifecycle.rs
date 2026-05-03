use chrono::Utc;
use rusqlite::Connection;
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
            .create_run(
                "persist-run",
                "hashXYZ",
                "runs/3/manifest.yaml",
                "kaggle",
                &[],
            )
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
fn migration_002_applies_on_top_of_001_without_data_loss() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("migration_test.db");

    // Create a v1 DB manually: run only migration 001 (without 002).
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(include_str!("../src/store/migrations/001_initial.sql"))
            .unwrap();
        conn.execute(
            "INSERT INTO runs \
             (id, name, manifest_hash, manifest_path, vendor, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
            rusqlite::params![
                "01HV0000000000000000000001",
                "legacy-run",
                "hash-v1",
                "path/manifest.yaml",
                "vast",
                "provisioning"
            ],
        )
        .unwrap();
    }

    // Open with the current Store (triggers migration from v1 to v2).
    let store = Store::open(&db_path).unwrap();

    // The run created under v1 schema must survive the migration.
    let runs = store.list_runs(&ListFilter::default()).unwrap();
    assert_eq!(runs.len(), 1, "run created before migration must persist");
    assert_eq!(runs[0].name, "legacy-run");
    assert_eq!(runs[0].vendor, "vast");
    assert_eq!(runs[0].status, RunStatus::Provisioning);
}

#[test]
fn poller_pid_roundtrip() {
    // poller_pid starts NULL, can be set, updated, and cleared.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("runs.db");
    let mut store = Store::open(&db_path).unwrap();

    let run_id = store
        .create_run("pid-run", "hash", "p.yaml", "vast", &[])
        .unwrap();

    let run = store.get_run(&run_id).unwrap().unwrap();
    assert_eq!(run.poller_pid, None, "fresh run has no poller_pid");

    store
        .update_run_poller_pid(&run_id, Some(12345))
        .expect("set pid");
    let run = store.get_run(&run_id).unwrap().unwrap();
    assert_eq!(run.poller_pid, Some(12345));

    store
        .update_run_poller_pid(&run_id, Some(99999))
        .expect("update pid");
    let run = store.get_run(&run_id).unwrap().unwrap();
    assert_eq!(run.poller_pid, Some(99999));

    store
        .update_run_poller_pid(&run_id, None)
        .expect("clear pid");
    let run = store.get_run(&run_id).unwrap().unwrap();
    assert_eq!(run.poller_pid, None);
}

#[test]
fn migration_004_applies_on_top_of_003_without_data_loss() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("migration_004.db");

    // Build a v3 schema by hand and seed a row.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(include_str!("../src/store/migrations/001_initial.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../src/store/migrations/002_cost_estimate.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../src/store/migrations/003_budget.sql"))
            .unwrap();
        conn.execute(
            "INSERT INTO runs \
             (id, name, manifest_hash, manifest_path, vendor, status, created_at) \
             VALUES (?1, 'legacy', 'h', 'p.yaml', 'vast', 'running', datetime('now'))",
            rusqlite::params!["01HV0000000000000000000099"],
        )
        .unwrap();
    }

    // Re-open: 004 runs and adds the column with NULL default.
    let mut store = Store::open(&db_path).unwrap();
    let runs = store.list_runs(&ListFilter::default()).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].poller_pid, None);

    // The column is writable post-migration.
    store
        .update_run_poller_pid(&runs[0].id, Some(42))
        .unwrap();
    let r = store.get_run(&runs[0].id).unwrap().unwrap();
    assert_eq!(r.poller_pid, Some(42));
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
