//! Real bridge, no network, no credentials: `ping` only imports `lightning_sdk`.
//! Skipped unless a Python with `lightning_sdk` is available.

use std::process::{Command, Stdio};

use xrun_core::config::credentials::LightningCredentials;
use xrun_core::pybridge::find_python;
use xrun_lightning::{LightningBridge, PyLightningBridge};

#[test]
fn ping_through_real_bridge() {
    let Some(py) = find_python() else {
        eprintln!("skip: no python interpreter");
        return;
    };
    let has_sdk = Command::new(py)
        .args(["-c", "import lightning_sdk"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_sdk {
        eprintln!("skip: lightning_sdk is not importable");
        return;
    }
    let bridge = PyLightningBridge::new(&LightningCredentials::default());
    let info = bridge.ping().expect("ping");
    assert!(!info.sdk_version.is_empty());
    // A second call reuses the same child.
    bridge.ping().expect("second ping");
}
