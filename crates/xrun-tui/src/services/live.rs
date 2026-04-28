#![deny(unsafe_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use xrun_core::{DataUpdate, ListFilter, RunId, RunStatus, Store};

/// Background service that monitors the DB every 5 seconds and sends DataUpdate
/// notifications to the TUI event loop. Also provides a SyncSender bridge so
/// synchronous pollers (xrun-poller) can notify the TUI without a tokio dependency.
pub struct LiveService {
    db_path: PathBuf,
    tx: mpsc::Sender<DataUpdate>,
    shutdown: Arc<AtomicBool>,
}

impl LiveService {
    pub fn new(db_path: PathBuf, tx: mpsc::Sender<DataUpdate>) -> Self {
        Self {
            db_path,
            tx,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Spawn the background watcher thread. Call once; non-blocking.
    pub fn start(self) {
        let db_path = self.db_path;
        let tx = self.tx;
        let shutdown = self.shutdown;

        std::thread::Builder::new()
            .name("xrun-live-service".to_string())
            .spawn(move || {
                let Ok(mut store) = Store::open(&db_path) else {
                    tracing::warn!("live service: cannot open DB at {}", db_path.display());
                    return;
                };
                let mut known: HashMap<RunId, (RunStatus, usize)> = HashMap::new();

                loop {
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    Self::poll_once(&mut store, &mut known, &tx);
                    // Sleep 5 s in 100 ms chunks so shutdown is responsive.
                    for _ in 0..50 {
                        if shutdown.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
    }

    /// Returns a `SyncSender` that bridges into the TUI channel.
    /// Pass this to `Poller::with_update_sender` so a running poller can push
    /// updates directly to the TUI without a tokio runtime dependency.
    pub fn make_poller_sender(tx: &mpsc::Sender<DataUpdate>) -> SyncSender<DataUpdate> {
        let (std_tx, std_rx) = std::sync::mpsc::sync_channel::<DataUpdate>(64);
        let tokio_tx = tx.clone();
        std::thread::Builder::new()
            .name("xrun-poller-bridge".to_string())
            .spawn(move || {
                while let Ok(update) = std_rx.recv() {
                    let _ = tokio_tx.try_send(update);
                }
            })
            .ok();
        std_tx
    }

    pub fn poll_once(
        store: &mut Store,
        known: &mut HashMap<RunId, (RunStatus, usize)>,
        tx: &mpsc::Sender<DataUpdate>,
    ) {
        let Ok(runs) = store.list_runs(&ListFilter::default()) else {
            return;
        };

        for run in &runs {
            let event_count = store.list_events(&run.id).map(|e| e.len()).unwrap_or(0);

            if let Some((prev_status, prev_count)) = known.get(&run.id) {
                if *prev_status != run.status {
                    let _ = tx.try_send(DataUpdate::RunStatusChanged(
                        run.id.clone(),
                        run.status.clone(),
                    ));
                }
                if event_count > *prev_count {
                    let _ = tx.try_send(DataUpdate::EventsAppended(
                        run.id.clone(),
                        event_count - prev_count,
                    ));
                }
            } else {
                let _ = tx.try_send(DataUpdate::RunCreated(run.id.clone()));
            }

            known.insert(run.id.clone(), (run.status.clone(), event_count));
        }
    }
}
