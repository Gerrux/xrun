use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;

#[test]
fn version_exit_0() {
    Command::cargo_bin("xrun")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("0.1.0"));
}

#[test]
fn config_init_creates_files() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .args(["config", "init"])
        .assert()
        .success()
        .stdout(contains("initialized"));

    assert!(dir.path().join("config.toml").exists());
    assert!(dir.path().join("credentials.toml").exists());
}

#[test]
fn config_init_idempotent() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .args(["config", "init"])
        .assert()
        .success();

    // Second init: must exit 0 and report "exists"
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .args(["config", "init"])
        .assert()
        .success()
        .stdout(contains("exists"));
}

#[test]
fn config_show_after_init_prints_defaults() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .args(["config", "init"])
        .assert()
        .success();

    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(contains("interval_active_secs"))
        .stdout(contains("interval_idle_secs"))
        .stdout(contains("<unset>"));
}
