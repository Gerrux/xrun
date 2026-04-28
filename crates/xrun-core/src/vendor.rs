#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::VendorError;
use crate::manifest::{DataSource, Manifest, RunSpec};
use crate::store::RunId;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
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

pub trait VendorAdapter {
    fn name(&self) -> &'static str;
    /// Associate a run ID so the adapter can link events/instances to the run.
    /// Default implementation is a no-op; adapters that write their own events override this.
    fn set_run_id(&self, _run_id: &RunId) {}
    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError>;
    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError>;
    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError>;
    fn upload(&self, h: &InstanceHandle, sources: &[DataSource]) -> Result<(), VendorError>;
    fn execute(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError>;
    fn tail(&self, h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError>;
    fn pull(&self, h: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError>;
    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError>;
}
