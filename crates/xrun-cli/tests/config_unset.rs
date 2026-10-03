//! `xrun config set --stdin` and `xrun config unset`: the write path the TUI
//! uses so secrets never appear in argv.

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::{tempdir, TempDir};

fn xrun(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("xrun").unwrap();
    cmd.env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"));
    cmd
}

fn init_dir() -> TempDir {
    let dir = tempdir().unwrap();
    xrun(&dir).args(["config", "init"]).assert().success();
    dir
}

fn creds(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap()
}

fn config(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.path().join("config.toml")).unwrap()
}

#[test]
fn set_stdin_stores_value_and_does_not_echo_it() {
    let dir = init_dir();
    let out = xrun(&dir)
        .args(["config", "set", "vast.api_key", "--stdin"])
        .write_stdin("test-key-abc\n")
        .assert()
        .success()
        .stdout(contains("vast.api_key: <set>"))
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("test-key-abc"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("test-key-abc"));
    assert!(creds(&dir).contains("api_key = \"test-key-abc\""));
}

#[test]
fn set_stdin_strips_only_one_trailing_newline() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "vast.api_key", "--stdin"])
        .write_stdin(" test-key-abc \r\n")
        .assert()
        .success();
    // Leading/trailing spaces survive, only the CRLF is dropped.
    assert!(creds(&dir).contains("api_key = \" test-key-abc \""));
}

#[test]
fn set_empty_credential_is_rejected_and_keeps_the_stored_one() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "vast.api_key", "--stdin"])
        .write_stdin("test-key-abc\n")
        .assert()
        .success();
    // `printf '%s' "$UNSET_VAR" | xrun config set … --stdin`
    for input in ["", "\n", "  \r\n"] {
        xrun(&dir)
            .args(["config", "set", "vast.api_key", "--stdin"])
            .write_stdin(input)
            .assert()
            .failure()
            .stderr(contains("config unset vast.api_key"));
    }
    xrun(&dir)
        .args(["config", "set", "vast.api_key", ""])
        .assert()
        .failure();
    assert!(creds(&dir).contains("api_key = \"test-key-abc\""));
}

#[test]
fn set_rejects_both_value_and_stdin() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "vast.api_key", "test-key-abc", "--stdin"])
        .write_stdin("other\n")
        .assert()
        .failure()
        .stderr(contains("not both"));
    assert!(!creds(&dir).contains("test-key-abc"));
}

#[test]
fn set_rejects_neither_value_nor_stdin() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "vast.api_key"])
        .assert()
        .failure()
        .stderr(contains("missing value"));
}

#[test]
fn unset_credential_key_clears_it() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "vast.api_key", "test-key-abc"])
        .assert()
        .success();
    xrun(&dir)
        .args(["config", "unset", "vast.api_key"])
        .assert()
        .success()
        .stdout(contains("vast.api_key: <unset>"));
    assert!(!creds(&dir).contains("test-key-abc"));
    xrun(&dir)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(contains("vast.api_key: <unset>"));
}

#[test]
fn unset_is_idempotent() {
    let dir = init_dir();
    for _ in 0..2 {
        xrun(&dir)
            .args(["config", "unset", "wandb.api_key"])
            .assert()
            .success()
            .stdout(contains("wandb.api_key: <unset>"));
    }
    xrun(&dir)
        .args(["config", "unset", "ssh.ghost"])
        .assert()
        .success();
    xrun(&dir)
        .args(["config", "unset", "ssh.ghost.port"])
        .assert()
        .success();
    xrun(&dir)
        .args(["config", "unset", "vendors.vast.default_gpu"])
        .assert()
        .success();
}

#[test]
fn unset_ssh_alias_removes_whole_entry() {
    let dir = init_dir();
    for (field, v) in [("host", "lab.example.com"), ("user", "alice")] {
        xrun(&dir)
            .args(["config", "set", &format!("ssh.lab.{field}"), v])
            .assert()
            .success();
    }
    xrun(&dir)
        .args(["config", "set", "ssh.other.host", "o.example.com"])
        .assert()
        .success();
    xrun(&dir)
        .args(["config", "unset", "ssh.lab"])
        .assert()
        .success()
        .stdout(contains("ssh.lab: <unset>"));
    let c = creds(&dir);
    assert!(
        !c.contains("[ssh.lab]") && !c.contains("lab.example.com"),
        "{c}"
    );
    assert!(c.contains("[ssh.other]"), "{c}");
}

#[test]
fn unset_ssh_field_clears_only_that_field() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "ssh.lab.host", "lab.example.com"])
        .assert()
        .success();
    xrun(&dir)
        .args(["config", "set", "ssh.lab.port", "2222"])
        .assert()
        .success();
    xrun(&dir)
        .args(["config", "unset", "ssh.lab.port"])
        .assert()
        .success();
    let c = creds(&dir);
    assert!(c.contains("host = \"lab.example.com\""), "{c}");
    assert!(!c.contains("port"), "{c}");
}

#[test]
fn unset_ssh_unknown_field_fails() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "unset", "ssh.lab.bogus"])
        .assert()
        .failure()
        .stderr(contains("unknown SSH field"));
}

#[test]
fn unset_global_key_resets_to_default() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "budget.max_lifetime_hours", "99"])
        .assert()
        .success();
    assert!(config(&dir).contains("max_lifetime_hours = 99"));
    xrun(&dir)
        .args(["config", "unset", "budget.max_lifetime_hours"])
        .assert()
        .success()
        .stdout(contains("budget.max_lifetime_hours: <unset>"));
    assert!(!config(&dir).contains("max_lifetime_hours = 99"));
}

#[test]
fn unset_vendor_field_resets_to_default() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "vendors.vast.default_gpu", "RTX_4090"])
        .assert()
        .success();
    assert!(config(&dir).contains("RTX_4090"));
    xrun(&dir)
        .args(["config", "unset", "vendors.vast.default_gpu"])
        .assert()
        .success()
        .stdout(contains("vendors.vast.default_gpu: <unset>"));
    assert!(!config(&dir).contains("RTX_4090"));
}

#[test]
fn unset_refuses_a_whole_section() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "set", "mlflow.url", "http://mlflow.example"])
        .assert()
        .success();
    xrun(&dir)
        .args(["config", "set", "budget.max_lifetime_hours", "99"])
        .assert()
        .success();
    for key in ["mlflow", "budget"] {
        xrun(&dir)
            .args(["config", "unset", key])
            .assert()
            .failure()
            .stderr(contains("cannot unset section"));
    }
    let c = config(&dir);
    assert!(c.contains("http://mlflow.example"), "{c}");
    assert!(c.contains("max_lifetime_hours = 99"), "{c}");
}

#[test]
fn unset_unknown_key_fails() {
    let dir = init_dir();
    xrun(&dir)
        .args(["config", "unset", "nope.nothing"])
        .assert()
        .failure()
        .stderr(contains("unknown config key"));
}
