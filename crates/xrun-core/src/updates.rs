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
}
