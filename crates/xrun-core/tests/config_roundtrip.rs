use pretty_assertions::assert_eq;
use tempfile::tempdir;
use xrun_core::{
    config::{ConfigStore, Credentials, GlobalConfig},
    manifest::Vendor,
};

#[test]
fn global_config_roundtrip() {
    let dir = tempdir().unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.mlflow.url = Some("http://mlflow.example.com".to_string());
    cfg.poller.interval_active_secs = 10;
    cfg.defaults.vendor = Some(Vendor::Vast);

    cfg.save(dir.path()).unwrap();
    let loaded = GlobalConfig::load(dir.path()).unwrap();
    assert_eq!(cfg, loaded);
}

#[test]
fn credentials_roundtrip() {
    let dir = tempdir().unwrap();
    let mut creds = Credentials::default();
    creds.vast.api_key = Some("test-key-abc".to_string());
    creds.mlflow.token = Some("tok-xyz".to_string());

    creds.save(dir.path()).unwrap();
    let loaded = Credentials::load(dir.path()).unwrap();
    assert_eq!(creds, loaded);
}

#[test]
fn mlflow_basic_auth_credentials_roundtrip() {
    // Username/password were added to support kaggle live-log streaming
    // through an MLflow tracking server protected by HTTP Basic auth.
    let dir = tempdir().unwrap();
    let mut creds = Credentials::default();
    creds.mlflow.username = Some("xrun".to_string());
    creds.mlflow.password = Some("hunter2".to_string());

    assert!(!creds.is_empty(), "auth fields must count toward is_empty");

    creds.save(dir.path()).unwrap();
    let loaded = Credentials::load(dir.path()).unwrap();
    assert_eq!(creds, loaded);
    assert_eq!(loaded.mlflow.username.as_deref(), Some("xrun"));
    assert_eq!(loaded.mlflow.password.as_deref(), Some("hunter2"));
}

#[test]
fn config_store_init_creates_files() {
    let dir = tempdir().unwrap();
    let result = ConfigStore::init(dir.path()).unwrap();
    assert!(!result.config_existed);
    assert!(!result.creds_existed);
    assert!(dir.path().join("config.toml").exists());
    assert!(dir.path().join("credentials.toml").exists());
}

#[test]
fn config_store_init_idempotent() {
    let dir = tempdir().unwrap();
    ConfigStore::init(dir.path()).unwrap();
    // Write a custom value so we can verify it is not overwritten.
    let mut cfg = GlobalConfig::load(dir.path()).unwrap();
    cfg.mlflow.url = Some("http://custom".to_string());
    cfg.save(dir.path()).unwrap();

    let result = ConfigStore::init(dir.path()).unwrap();
    assert!(result.config_existed);
    assert!(result.creds_existed);

    let after = GlobalConfig::load(dir.path()).unwrap();
    assert_eq!(after.mlflow.url.as_deref(), Some("http://custom"));
}

#[test]
fn credentials_is_empty_detects_unset() {
    let creds = Credentials::default();
    assert!(creds.is_empty());

    let mut creds = Credentials::default();
    creds.vast.api_key = Some("x".to_string());
    assert!(!creds.is_empty());
}

#[cfg(unix)]
#[test]
fn credentials_file_is_owner_readable_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let mut creds = Credentials::default();
    creds.vast.api_key = Some("test-key-abc".to_string());
    creds.save(dir.path()).unwrap();

    let meta = std::fs::metadata(dir.path().join("credentials.toml")).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credentials.toml must be 0600, got {:o}", mode);
}

#[cfg(windows)]
#[test]
fn credentials_file_dacl_is_locked_down_on_windows() {
    // Verifies that `Credentials::save` invokes icacls and the resulting
    // DACL has no inherited ACEs (inheritance was stripped) and grants
    // access only to the current user. We shell out to `icacls` to read
    // back the ACL because parsing raw security descriptors would require
    // unsafe Win32 calls, which the crate forbids.
    let dir = tempdir().unwrap();
    let mut creds = Credentials::default();
    creds.vast.api_key = Some("test-key-abc".to_string());
    creds.save(dir.path()).unwrap();

    let path = dir.path().join("credentials.toml");
    let output = std::process::Command::new("icacls")
        .arg(&path)
        .output()
        .expect("icacls must be available on Windows");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stdout.contains("BUILTIN\\Users")
            && !stdout.contains("Everyone")
            && !stdout.contains("Authenticated Users"),
        "credentials.toml DACL must not grant access to broad groups, got:\n{}",
        stdout
    );

    let username = std::env::var("USERNAME").unwrap_or_default();
    assert!(
        stdout.contains(&username),
        "credentials.toml DACL must grant access to current user '{}', got:\n{}",
        username,
        stdout
    );
}

#[test]
fn global_config_loads_with_defaults_when_missing() {
    let dir = tempdir().unwrap();
    let cfg = GlobalConfig::load(dir.path()).unwrap();
    assert_eq!(cfg.poller.interval_active_secs, 5);
    assert_eq!(cfg.poller.interval_idle_secs, 30);
    assert!(cfg.mlflow.url.is_none());
}
