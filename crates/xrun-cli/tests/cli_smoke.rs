use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn version_exit_0() {
    Command::cargo_bin("xrun")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("0.1.0"));
}
