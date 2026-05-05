#![deny(unsafe_code)]

//! WandB sink — see `sink::WandbSink` for the `MetricSink` impl, and
//! `client::WandbClient` for the underlying GraphQL + file_stream wrapper.

pub mod client;
pub mod error;
pub mod sink;
pub mod types;

pub use client::{WandbClient, DEFAULT_API_BASE, DEFAULT_WEB_BASE};
pub use error::WandbError;
pub use sink::WandbSink;
pub use types::{ExitCode, HistoryLine, WandbRunInfo};
