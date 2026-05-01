#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::VendorError;
use crate::manifest::{DataSource, Manifest, RunSpec};
use crate::store::{RunId, RunStatus};

/// Snapshot of a vendor's account state, surfaced to TUI/CLI.
/// `connected = false` means the credentials are missing or rejected.
/// `balance` is in `currency` units when both are set; for vast it's USD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VendorStatus {
    pub connected: bool,
    pub balance: Option<f64>,
    pub currency: Option<String>,
    pub account: Option<String>,
    pub last_checked: DateTime<Utc>,
    pub error: Option<String>,
}

/// Vendor-side view of a running/queued machine, surfaced in the Instances
/// screen alongside the locally-tracked ones. Adapters fill what they can;
/// missing fields render as "—".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VendorRemoteInstance {
    pub id: String,
    pub gpu: Option<String>,
    pub num_gpus: Option<u32>,
    pub dph_total: Option<f64>,
    pub status: Option<String>,
    pub uptime_secs: Option<u64>,
    pub ssh: Option<String>,
    pub region: Option<String>,
}

impl VendorStatus {
    pub fn not_configured() -> Self {
        Self {
            connected: false,
            balance: None,
            currency: None,
            account: None,
            last_checked: Utc::now(),
            error: Some("not configured".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceHandle {
    pub id: String,
    pub vendor: String,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_user: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DryRunPlan {
    pub gpu_query: String,
    pub estimated_price_max: f64,
    pub data_total_bytes: u64,
    pub data_items: Vec<(PathBuf, String)>,
    pub cmd_line: String,
}

/// A synthetic lifecycle event emitted by `poll_completion` (Kaggle and similar vendors
/// that do not support live JSONL streaming).
pub struct SyntheticEvent {
    pub stage: String,
    /// "start" | "ok" | "fail" | "progress"
    pub status: String,
    pub msg: Option<String>,
}

/// Return value of `VendorAdapter::poll_completion`. Carries zero or more
/// synthetic events to write to the store, plus an optional terminal status.
/// When `terminal_status` is `Some`, the Poller stops.
pub struct PollCompletion {
    /// `None` = still running (but events may still be present).
    pub terminal_status: Option<RunStatus>,
    pub events: Vec<SyntheticEvent>,
}

pub trait VendorAdapter {
    fn name(&self) -> &'static str;
    /// Associate a run ID so the adapter can link events/instances to the run.
    /// Default implementation is a no-op; adapters that write their own events override this.
    fn set_run_id(&self, _run_id: &RunId) {}
    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError>;
    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError>;
    /// Probe the vendor's account state (balance, account name, reachability).
    /// Default impl returns `NotImplemented`; adapters override to call their CLI/API.
    fn vendor_status(&self) -> Result<VendorStatus, VendorError> {
        Err(VendorError::NotImplemented)
    }
    /// List the user's running/queued machines from the vendor's side.
    /// Default impl returns `NotImplemented`; adapters override.
    fn vendor_instances(&self) -> Result<Vec<VendorRemoteInstance>, VendorError> {
        Err(VendorError::NotImplemented)
    }
    /// Called on each Poller tick for vendors that use completion-poll instead of
    /// live JSONL streaming (e.g. Kaggle). Returns `Some` when there is a state
    /// change or terminal condition; returns `None` to skip (no new info).
    /// Default: always `None` (vast uses the normal `tail()` path).
    fn poll_completion(&self, _h: &InstanceHandle, _run_dir: &Path) -> Option<PollCompletion> {
        None
    }
    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError>;
    fn upload(&self, h: &InstanceHandle, sources: &[DataSource]) -> Result<(), VendorError>;
    fn execute(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError>;
    fn tail(&self, h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError>;
    fn pull(&self, h: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError>;
    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError>;

    /// Liveness probe for the actual training subprocess. Adapters that can
    /// capture the child PID (vast and ssh do via `nohup … & echo $! > pid`)
    /// override this to return `Some(true)` while the PID is alive,
    /// `Some(false)` once it's gone. Returning `None` means the adapter has
    /// no PID-tracking — the poller falls back to its idle-timer heuristic.
    ///
    /// This is what closes the "process death → idle_timeout misclassification"
    /// gap from Issue 2 — when the python child gets OOM-killed but bash and
    /// sshd are still up, idle-tracking can't tell the difference. PID
    /// liveness can.
    fn process_alive(&self, _h: &InstanceHandle) -> Option<bool> {
        None
    }
}
