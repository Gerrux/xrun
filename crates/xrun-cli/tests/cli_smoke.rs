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
        .stdout(contains("xrun"));
}

#[test]
fn config_init_creates_files() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
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
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "init"])
        .assert()
        .success();

    // Second init: must exit 0 and report "exists"
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
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
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "init"])
        .assert()
        .success();

    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(contains("interval_active_secs"))
        .stdout(contains("interval_idle_secs"))
        .stdout(contains("<unset>"));
}

// ── xrun init ──────────────────────────────────────────────────────────────

#[test]
fn init_probe_local_emits_json() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["init", "--probe-local", "--json"])
        .assert()
        .success()
        .stdout(contains("\"os\""))
        .stdout(contains("\"gpus\""));
}

#[test]
fn init_non_interactive_marks_completed_and_writes_sinks() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args([
            "init",
            "--non-interactive",
            "--mark-completed",
            "--sink",
            "mlflow",
        ])
        .assert()
        .success();

    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("wizard_completed = true"),
        "config.toml missing wizard_completed flag:\n{toml}"
    );
    assert!(
        toml.contains("sinks = [\"mlflow\"]"),
        "config.toml missing metrics.sinks:\n{toml}"
    );
}

#[test]
fn init_non_interactive_writes_vast_key() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args([
            "init",
            "--non-interactive",
            "--mark-completed",
            "--vast-key",
            "test-key-abc123",
        ])
        .assert()
        .success()
        .stdout(contains("vast.api_key"));

    let creds = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(
        creds.contains("api_key = \"test-key-abc123\""),
        "credentials.toml missing vast api_key:\n{creds}"
    );
}

#[test]
fn init_rejects_kaggle_username_without_key() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args([
            "init",
            "--non-interactive",
            "--kaggle-username",
            "alice",
        ])
        .assert()
        .failure()
        .stderr(contains("--kaggle-username requires --kaggle-key"));
}

#[test]
fn init_rejects_credential_flags_without_non_interactive() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["init", "--vast-key", "x"])
        .assert()
        .failure()
        .stderr(contains("require --non-interactive"));
}

#[test]
fn config_set_ssh_host_writes_credentials() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "init"])
        .assert()
        .success();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "set", "ssh.lab.host", "lab.example.com"])
        .assert()
        .success()
        .stdout(contains("ssh.lab.host"));
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "set", "ssh.lab.user", "alice"])
        .assert()
        .success();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "set", "ssh.lab.port", "2222"])
        .assert()
        .success();
    let creds = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(creds.contains("[ssh.lab]"), "missing [ssh.lab]:\n{creds}");
    assert!(creds.contains("host = \"lab.example.com\""), "{creds}");
    assert!(creds.contains("user = \"alice\""), "{creds}");
    assert!(creds.contains("port = 2222"), "{creds}");
}

#[test]
fn config_set_ssh_rejects_bad_alias() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "set", "ssh.bad alias.host", "x"])
        .assert()
        .failure()
        .stderr(contains("invalid SSH alias"));
}

#[test]
fn init_non_interactive_rejects_unknown_sink() {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["init", "--non-interactive", "--sink", "wandb"])
        .assert()
        .failure()
        .stderr(contains("unknown sink"));
}
