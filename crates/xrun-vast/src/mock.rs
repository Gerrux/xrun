#![deny(unsafe_code)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;

use xrun_core::{
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};

/// A scripted VendorAdapter for integration and end-to-end tests.
///
/// Supply batches of raw JSONL bytes for events and metrics tails. Each call to
/// `tail` pops the next batch off the respective queue; an empty queue returns
/// an empty byte slice (no more data).
pub struct MockVastAdapter {
    events_queue: RefCell<VecDeque<Vec<u8>>>,
    metrics_queue: RefCell<VecDeque<Vec<u8>>>,
}

impl MockVastAdapter {
    pub fn new(events: Vec<Vec<u8>>, metrics: Vec<Vec<u8>>) -> Self {
        Self {
            events_queue: RefCell::new(events.into()),
            metrics_queue: RefCell::new(metrics.into()),
        }
    }
}

impl VendorAdapter for MockVastAdapter {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn validate(&self, _: &Manifest) -> Result<(), VendorError> {
        Ok(())
    }

    fn dry_run_plan(&self, _: &Manifest) -> Result<DryRunPlan, VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn provision(&self, _: &Manifest) -> Result<InstanceHandle, VendorError> {
        Ok(InstanceHandle {
            id: "mock-999".to_string(),
            vendor: "mock".to_string(),
            ssh_host: Some("127.0.0.1".to_string()),
            ssh_port: Some(22),
            ssh_user: "root".to_string(),
        })
    }

    fn upload(&self, _: &InstanceHandle, _: &[DataSource]) -> Result<(), VendorError> {
        Ok(())
    }

    fn execute(&self, _: &InstanceHandle, _: &RunSpec) -> Result<(), VendorError> {
        Ok(())
    }

    fn tail(&self, _h: &InstanceHandle, file: &str, _offset: u64) -> Result<Vec<u8>, VendorError> {
        let queue = if file.contains("metrics") {
            &self.metrics_queue
        } else {
            &self.events_queue
        };
        Ok(queue.borrow_mut().pop_front().unwrap_or_default())
    }

    fn pull(&self, _: &InstanceHandle, _: &str, _: &Path) -> Result<(), VendorError> {
        Ok(())
    }

    fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
        Ok(())
    }
}
