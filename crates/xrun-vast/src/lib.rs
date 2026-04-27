#![deny(unsafe_code)]

//! Vast.ai vendor adapter for xrun.

pub mod cli;
pub mod error;
pub mod process;
pub mod stub;

pub use stub::VastStub;
