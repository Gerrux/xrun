//! Live integration: invokes real `vastai` with a deliberately bad api key.
//! Must remain `#[ignore]` by default so CI without `vastai` in PATH stays green.

use tempfile::tempdir;
use xrun_core::config::credentials::VastCredentials;
use xrun_core::vendor::VendorAdapter;
use xrun_core::Store;
use xrun_vast::VastAdapter;

#[test]
#[ignore = "requires vastai binary in PATH"]
fn vendor_status_with_bad_key_returns_auth_error() {
    let dir = tempdir().unwrap();
    let store = Store::open(&dir.path().join("xrun.db")).unwrap();
    let creds = VastCredentials {
        api_key: Some("definitely-not-a-real-key-1234567890abcdef".to_string()),
    };
    let adapter = VastAdapter::new(creds, store);

    let status = adapter
        .vendor_status()
        .expect("vendor_status must not panic");
    assert!(!status.connected, "bad key must report connected=false");
    let err = status.error.expect("an error must be reported");
    assert!(
        err.to_lowercase().contains("rejected") || err.contains("403") || err.contains("401"),
        "expected auth-failure phrasing in error, got: {}",
        err
    );
    assert!(
        err.contains("cloud.vast.ai"),
        "expected the help URL in the error, got: {}",
        err
    );
}

#[test]
#[ignore = "requires vastai binary in PATH"]
fn vendor_status_without_key_skips_cli() {
    let dir = tempdir().unwrap();
    let store = Store::open(&dir.path().join("xrun.db")).unwrap();
    let adapter = VastAdapter::new(VastCredentials { api_key: None }, store);

    let status = adapter.vendor_status().expect("must not error");
    assert!(!status.connected);
    assert_eq!(status.error.as_deref(), Some("api_key not set"));
}
