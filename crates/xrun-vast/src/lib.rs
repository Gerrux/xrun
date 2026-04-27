#![deny(unsafe_code)]

//! Vast.ai vendor adapter for xrun.

pub mod adapter;
pub mod stub;

pub use stub::VastStub;
