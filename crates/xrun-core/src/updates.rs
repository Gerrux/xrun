#![deny(unsafe_code)]

use crate::store::{RunId, RunStatus};

#[derive(Debug, Clone)]
pub enum DataUpdate {
    RunCreated(RunId),
    RunStatusChanged(RunId, RunStatus),
    EventsAppended(RunId, usize),
    MetricsAppended(RunId, usize),
    LogsAppended(RunId, u64),
    InstanceUpdated(String),
    /// A vendor's account status (balance/connectivity) has been refreshed.
    /// Carries the vendor name; new VendorStatus is read from app state.
    VendorStatusUpdated(String),
    /// A vendor's remote instance list has been refreshed.
    VendorInstancesUpdated(String),
}
