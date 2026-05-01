#![deny(unsafe_code)]

pub mod adapter;
pub mod cli;
pub mod embed;
pub mod error;
pub mod http;
pub mod ingest;
pub mod kernel_metadata;

pub use adapter::KaggleAdapter;
pub use cli::{
    DatasetListItem, KaggleCli, KaggleProcess, KernelListItem, KernelState, KernelStatus,
};
pub use error::KaggleError;
pub use http::{auth_from_credentials, Auth, CancelOutcome, KaggleApiClient};
pub use kernel_metadata::KernelMetadata;
