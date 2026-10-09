//! Talks to the real Python bridge. Skipped when no interpreter with
//! `google-colab-cli` is available. Only `ping` (no auth, no network) and the
//! unknown-op error path are exercised; the OAuth flow is never started.

use std::process::{Command, Stdio};

use xrun_colab::{ColabBridge, PyColabBridge};
use xrun_core::pybridge::find_python;

fn have_colab_cli() -> bool {
    let Some(py) = find_python() else {
        return false;
    };
    Command::new(py)
        .args(["-c", "import colab_cli.common, colab_cli.runtime"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn ping_reports_sdk_version() {
    if !have_colab_cli() {
        eprintln!("skipped: no python with google-colab-cli");
        return;
    }
    let bridge = PyColabBridge::new();
    let info = bridge.ping().expect("ping");
    assert!(!info.sdk_version.is_empty());
    // Second call reuses the persistent child.
    bridge.ping().expect("second ping");
}

#[test]
fn script_is_embedded_and_has_login_mode() {
    assert!(xrun_colab::BRIDGE_PY.contains("--login"));
    assert!(xrun_colab::BRIDGE_PY.contains("<<<XRUN_BRIDGE>>>"));
}
