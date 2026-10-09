#![deny(unsafe_code)]

//! Google Colab vendor adapter for xrun.
//!
//! A Colab runtime session is the box. The only SDK is Python
//! (`google-colab-cli`, package `colab_cli`), so all calls go through one
//! persistent Python child ([`bridge::PyColabBridge`], script `bridge.py`).
//! Shell work, tail, glob and process probes run as small Python snippets in
//! the session kernel ([`cmd`]); files move through the Jupyter contents API.
//!
//! Limits: `upload` reads each file whole into memory (base64), so it suits
//! small data; sessions live at most ~12 h and GPUs are "when available".

pub mod adapter;
pub mod bridge;
pub mod cmd;
pub mod error;

pub use adapter::{ColabAdapter, DEFAULT_GPU, DEFAULT_WORKDIR};
pub use bridge::{
    run_login, ColabBridge, ExecOutput, PingInfo, PyColabBridge, SessionInfo, WhoAmI, BRIDGE_PY,
};
#[cfg(any(test, feature = "mock"))]
pub use bridge::{FakeBridge, FakeCall};
pub use error::ColabError;

#[cfg(test)]
mod adapter_tests;
