#![deny(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use std::sync::Mutex;

use tokio::sync::mpsc;
use xrun_core::vendor::{VendorAdapter, VendorStatus};
use xrun_core::DataUpdate;

pub enum ProbeRequest {
    /// Re-probe all configured vendors.
    All,
    /// Re-probe a single vendor by name.
    Vendor(String),
}

pub struct VendorProbeService {
    adapters: Vec<(String, Box<dyn VendorAdapter + Send>)>,
    tx: mpsc::Sender<DataUpdate>,
    cmd_tx: std_mpsc::Sender<ProbeRequest>,
    cmd_rx: Mutex<Option<std_mpsc::Receiver<ProbeRequest>>>,
    shutdown: Arc<AtomicBool>,
}

impl VendorProbeService {
    pub fn new(
        adapters: Vec<(String, Box<dyn VendorAdapter + Send>)>,
        tx: mpsc::Sender<DataUpdate>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = std_mpsc::channel();
        Self {
            adapters,
            tx,
            cmd_tx,
            cmd_rx: Mutex::new(Some(cmd_rx)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    pub fn command_sender(&self) -> std_mpsc::Sender<ProbeRequest> {
        self.cmd_tx.clone()
    }

    pub fn start(self) {
        let adapters = self.adapters;
        let tx = self.tx;
        let cmd_rx = self.cmd_rx.lock().ok().and_then(|mut g| g.take());
        let Some(cmd_rx) = cmd_rx else {
            tracing::warn!("vendor probe: command receiver missing");
            return;
        };
        let shutdown = self.shutdown;
        let interval = Duration::from_secs(60);
        let initial_delay = Duration::from_millis(200);

        if let Err(e) = std::thread::Builder::new()
            .name("xrun-vendor-probe".to_string())
            .spawn(move || {
                std::thread::sleep(initial_delay);
                Self::probe_all(&adapters, &tx);

                let mut last = std::time::Instant::now();
                loop {
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    // Drain commands with a short timeout so periodic ticks still fire.
                    match cmd_rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(ProbeRequest::All) => {
                            Self::probe_all(&adapters, &tx);
                            last = std::time::Instant::now();
                        }
                        Ok(ProbeRequest::Vendor(name)) => {
                            if let Some((n, ad)) = adapters.iter().find(|(n, _)| n == &name) {
                                Self::probe_one(n, ad.as_ref(), &tx);
                            }
                        }
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {
                            if last.elapsed() >= interval {
                                Self::probe_all(&adapters, &tx);
                                last = std::time::Instant::now();
                            }
                        }
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
        {
            tracing::warn!("failed to spawn vendor-probe thread: {}", e);
        }
    }

    fn probe_all(
        adapters: &[(String, Box<dyn VendorAdapter + Send>)],
        tx: &mpsc::Sender<DataUpdate>,
    ) {
        for (name, ad) in adapters {
            Self::probe_one(name, ad.as_ref(), tx);
        }
    }

    fn probe_one(name: &str, adapter: &dyn VendorAdapter, tx: &mpsc::Sender<DataUpdate>) {
        let status = match adapter.vendor_status() {
            Ok(s) => s,
            Err(e) => VendorStatus {
                connected: false,
                balance: None,
                currency: None,
                account: None,
                last_checked: chrono::Utc::now(),
                error: Some(e.to_string()),
            },
        };
        // Latest status is stashed in a module-global map; the app reads it
        // by name when it sees DataUpdate::VendorStatusUpdated.
        latest::write(name, status);
        let _ = tx.try_send(DataUpdate::VendorStatusUpdated(name.to_string()));

        // Instance list is best-effort: failure here doesn't block the status
        // signal. Errors get logged at debug only.
        match adapter.vendor_instances() {
            Ok(insts) => {
                latest::write_instances(name, insts);
                let _ = tx.try_send(DataUpdate::VendorInstancesUpdated(name.to_string()));
            }
            Err(e) => {
                tracing::debug!("vendor_instances({}) failed: {}", name, e);
            }
        }
    }
}

/// Module-global latest snapshot of every vendor's last probe result.
/// Read by `app::on_data_update` to update `AppState.vendor_statuses`.
pub mod latest {
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};

    use xrun_core::vendor::{VendorRemoteInstance, VendorStatus};

    static STATUS_MAP: OnceLock<RwLock<HashMap<String, VendorStatus>>> = OnceLock::new();
    static INSTANCES_MAP: OnceLock<RwLock<HashMap<String, Vec<VendorRemoteInstance>>>> =
        OnceLock::new();

    fn status_map() -> &'static RwLock<HashMap<String, VendorStatus>> {
        STATUS_MAP.get_or_init(|| RwLock::new(HashMap::new()))
    }

    fn instances_map() -> &'static RwLock<HashMap<String, Vec<VendorRemoteInstance>>> {
        INSTANCES_MAP.get_or_init(|| RwLock::new(HashMap::new()))
    }

    pub fn write(name: &str, s: VendorStatus) {
        if let Ok(mut g) = status_map().write() {
            g.insert(name.to_string(), s);
        }
    }

    pub fn read(name: &str) -> Option<VendorStatus> {
        status_map().read().ok().and_then(|g| g.get(name).cloned())
    }

    pub fn write_instances(name: &str, insts: Vec<VendorRemoteInstance>) {
        if let Ok(mut g) = instances_map().write() {
            g.insert(name.to_string(), insts);
        }
    }

    pub fn read_instances(name: &str) -> Option<Vec<VendorRemoteInstance>> {
        instances_map()
            .read()
            .ok()
            .and_then(|g| g.get(name).cloned())
    }
}
