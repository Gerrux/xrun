#![deny(unsafe_code)]

//! Core library for xrun: manifest parsing, SQLite storage, config, and domain types.

pub mod budget;
pub mod config;
pub mod error;
pub mod events;
pub mod manifest;
pub mod metrics;
pub mod paths;
pub mod store;
pub mod updates;
pub mod vendor;

pub use budget::{accumulate_cost, caps_from_config, evaluate_caps, DestroyReason};
pub use config::{BudgetConfig, ConfigStore, Credentials, GlobalConfig, InitResult};
pub use error::{ConfigError, JsonlError, ManifestError, StoreError, VendorError};
pub use events::{Event, EventStatus, JsonlReader};
pub use manifest::Manifest;
pub use metrics::{Metric, MetricsJsonlReader};
pub use store::{
    Instance, InstanceCaps, ListFilter, Run, RunId, RunStatus, Store, StoredEvent, StoredMetric,
};
pub use updates::DataUpdate;
pub use vendor::{
    DryRunPlan, InstanceHandle, PollCompletion, SyntheticEvent, VendorAdapter,
    VendorRemoteInstance, VendorStatus,
};
