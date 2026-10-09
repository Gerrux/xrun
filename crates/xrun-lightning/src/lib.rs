#![deny(unsafe_code)]

//! Lightning AI vendor adapter. A Lightning "Studio" is the box: it is
//! started on `provision`, driven through the `lightning-sdk` Python package
//! (a persistent child process, see `bridge.py`), and stopped on `destroy`.
//! The training run itself uses the same remote layout as the ssh adapter
//! (`events.jsonl`, `metrics.jsonl`, `stdout.log`, `run.pid` under the run dir).

pub mod adapter;
pub mod bridge;
pub mod cmd;
pub mod error;

pub use adapter::LightningAdapter;
pub use bridge::{
    LightningBridge, PyLightningBridge, RunOut, SdkInfo, StartParams, StudioEntry, StudioInfo,
    StudioRef, WhoAmI,
};
pub use cmd::studio_name_for;
pub use error::LightningError;
