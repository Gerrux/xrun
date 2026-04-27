#![deny(unsafe_code)]

//! Core library for xrun: manifest parsing, SQLite storage, config, and domain types.

pub mod config;
pub mod error;
pub mod events;
pub mod manifest;
pub mod metrics;
pub mod paths;
pub mod store;
pub mod vendor;

pub use config::{ConfigStore, Credentials, GlobalConfig, InitResult};
pub use error::{ConfigError, JsonlError, ManifestError, StoreError, VendorError};
pub use events::{Event, EventStatus, JsonlReader, StdStage};
pub use manifest::Manifest;
pub use metrics::{Metric, MetricsJsonlReader};
pub use store::{Run, RunId, RunStatus, Store};
pub use vendor::{DryRunPlan, InstanceHandle, VendorAdapter};
