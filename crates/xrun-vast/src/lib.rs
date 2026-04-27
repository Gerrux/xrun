#![deny(unsafe_code)]

//! Vast.ai vendor adapter for xrun.

pub mod adapter;
pub mod cli;
pub mod error;
pub mod execute;
pub mod process;
pub mod provision;
pub mod stub;
pub mod upload;

pub use adapter::VastAdapter;
pub use stub::VastStub;
