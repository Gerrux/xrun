/// Integration tests for the LiveService DB-polling channel.
///
/// The LiveService polls the DB every 5 s; these tests drive `poll_once`
/// directly so they don't have to sleep.
use std::collections::HashMap;

use chrono::Utc;
use tempfile::tempdir;
use tokio::sync::mpsc;
use xrun_core::{DataUpdate, RunId, RunStatus, Store};
use xrun_tui::services::live::LiveService;

fn open_store(dir: &std::path::Path) -> Store {
    Store::open(&dir.join("test.db")).expect("open store")
}

// Helper: drain all pending messages from an unbounded-style bounded channel.
fn drain(rx: &mut mpsc::Receiver<DataUpdate>) -> Vec<DataUpdate> {
    let mut out = Vec::new();
    while let Ok(u) = rx.try_recv() {
        out.push(u);
    }
    out
}

#[test]
fn poll_once_detects_new_run_as_run_created() {
    let dir = tempdir().unwrap();
    let mut store = open_store(dir.path());
    let (tx, mut rx) = mpsc::channel::<DataUpdate>(256);
    let mut known = HashMap::new();

    let _id = store
        .create_run("my-run", "hash1", "exp/my-run.yaml", "vast", &[])
        .unwrap();

    LiveService::poll_once(&mut store, &mut known, &tx);

    let updates = drain(&mut rx);
    assert_eq!(updates.len(), 1, "expected exactly one update");
    assert!(
        matches!(updates[0], DataUpdate::RunCreated(_)),
        "expected RunCreated, got {:?}",
        updates[0]
    );
}

#[test]
fn poll_once_detects_status_change() {
    let dir = tempdir().unwrap();
    let mut store = open_store(dir.path());
    let (tx, mut rx) = mpsc::channel::<DataUpdate>(256);
    let mut known = HashMap::new();

    let id = store
        .create_run("run-a", "h", "exp/a.yaml", "vast", &[])
        .unwrap();

    // First poll: learn about the run.
    LiveService::poll_once(&mut store, &mut known, &tx);
    drain(&mut rx); // discard RunCreated

    // Change status, then poll again.
    store.update_run_status(&id, RunStatus::Running).unwrap();
    LiveService::poll_once(&mut store, &mut known, &tx);

    let updates = drain(&mut rx);
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, DataUpdate::RunStatusChanged(_, RunStatus::Running))),
        "expected RunStatusChanged(Running), got {:?}",
        updates
    );
}

#[test]
fn poll_once_detects_events_appended() {
    let dir = tempdir().unwrap();
    let mut store = open_store(dir.path());
    let (tx, mut rx) = mpsc::channel::<DataUpdate>(256);
    let mut known = HashMap::new();

    let id = store
        .create_run("run-b", "h2", "exp/b.yaml", "vast", &[])
        .unwrap();

    // First poll.
    LiveService::poll_once(&mut store, &mut known, &tx);
    drain(&mut rx);

    // Append 5 events, then poll again.
    for i in 0..5u32 {
        store
            .append_event(
                &id,
                xrun_core::store::NewEvent {
                    ts: Utc::now(),
                    stage: format!("stage-{i}"),
                    status: "ok".to_string(),
                    msg: None,
                    payload_json: None,
                },
            )
            .unwrap();
    }

    LiveService::poll_once(&mut store, &mut known, &tx);

    let updates = drain(&mut rx);
    let appended: Vec<_> = updates
        .iter()
        .filter(|u| matches!(u, DataUpdate::EventsAppended(_, _)))
        .collect();
    assert_eq!(appended.len(), 1, "expected one EventsAppended update");
    if let DataUpdate::EventsAppended(_, count) = appended[0] {
        assert_eq!(*count, 5, "expected 5 new events, got {count}");
    }
}

#[test]
fn poll_once_no_spurious_updates_when_nothing_changes() {
    let dir = tempdir().unwrap();
    let mut store = open_store(dir.path());
    let (tx, mut rx) = mpsc::channel::<DataUpdate>(256);
    let mut known = HashMap::new();

    let _id = store
        .create_run("run-c", "h3", "exp/c.yaml", "vast", &[])
        .unwrap();

    LiveService::poll_once(&mut store, &mut known, &tx);
    drain(&mut rx); // discard RunCreated

    // Poll again with no changes.
    LiveService::poll_once(&mut store, &mut known, &tx);
    let updates = drain(&mut rx);
    assert!(
        updates.is_empty(),
        "expected no updates when nothing changed, got {:?}",
        updates
    );
}

#[test]
fn make_poller_sender_bridges_to_tokio_channel() {
    // Basic smoke test: messages sent through the SyncSender arrive on the tokio channel.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (tx, mut rx) = mpsc::channel::<DataUpdate>(64);
        let std_tx = LiveService::make_poller_sender(&tx);

        let id = RunId::new();
        std_tx
            .try_send(DataUpdate::RunCreated(id.clone()))
            .expect("SyncSender try_send");

        // Give the bridge thread a moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let update = rx.try_recv().expect("should have received update");
        assert!(
            matches!(update, DataUpdate::RunCreated(_)),
            "unexpected update: {:?}",
            update
        );
    });
}
