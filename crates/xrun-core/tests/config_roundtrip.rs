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

#[test]
fn global_config_loads_with_defaults_when_missing() {
    let dir = tempdir().unwrap();
    let cfg = GlobalConfig::load(dir.path()).unwrap();
    assert_eq!(cfg.poller.interval_active_secs, 5);
    assert_eq!(cfg.poller.interval_idle_secs, 30);
    assert!(cfg.mlflow.url.is_none());
}
