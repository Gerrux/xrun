#![deny(unsafe_code)]

//! Vast.ai vendor adapter for xrun.

pub mod adapter;
pub mod cli;
pub mod error;
pub mod execute;
#[cfg(feature = "mock")]
pub mod mock;
pub mod process;
pub mod provision;
pub mod pull;
pub mod stub;
pub mod tail;
pub mod upload;

pub use adapter::VastAdapter;
#[cfg(feature = "mock")]
pub use mock::MockVastAdapter;
pub use stub::VastStub;
