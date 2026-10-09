//! CLI wiring of the Lightning AI and Google Colab vendors. None of these
//! tests needs python, the SDKs or a network: bridge failures must surface as
//! `ok: false` rows / JSON, never as a crash.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn template(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../exp/templates")
        .join(name)
}

fn xrun(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("xrun").unwrap();
    cmd.env("XRUN_DATA_DIR", tmp.path().join("data"))
        .env("XRUN_CONFIG_DIR", tmp.path().join("config"))
        // A missing interpreter must degrade to ok:false, not hang or crash.
        .env("XRUN_PYTHON", "definitely-not-a-python-binary");
    cmd
}

#[test]
fn launch_dry_run_lightning_template() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .arg("launch")
        .arg(template("lightning_smoke.yaml"))
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("gpu_query"));
}

#[test]
fn launch_dry_run_colab_template() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .arg("launch")
        .arg(template("colab_smoke.yaml"))
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("gpu_query"));
}

#[test]
fn init_manifest_lightning_and_colab_parse() {
    for vendor in ["lightning", "colab"] {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join(format!("{vendor}.yaml"));
        xrun(&tmp)
            .args(["init-manifest", "--vendor", vendor, "--into"])
            .arg(&out)
            .assert()
            .success();
        let body = std::fs::read_to_string(&out).unwrap();
        assert!(body.contains(&format!("vendor: {vendor}")), "{body}");
        xrun_core::manifest::Manifest::from_yaml_str(&body)
            .unwrap_or_else(|e| panic!("{vendor} skeleton must validate: {e}"));
        // The generated skeleton must also pass a dry-run end to end.
        xrun(&tmp)
            .arg("launch")
            .arg(&out)
            .arg("--dry-run")
            .assert()
            .success();
    }
}

#[test]
fn init_manifest_unknown_vendor_lists_all_six() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .args(["init-manifest", "--vendor", "runpod", "--into", "-"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "vast, kaggle, local, ssh, lightning, colab",
        ));
}

#[test]
fn doctor_all_json_has_the_four_new_rows() {
    let tmp = TempDir::new().unwrap();
    let out = xrun(&tmp)
        .args(["doctor", "--all", "--json"])
        .output()
        .unwrap();
    // Exit code may be 1 (failing checks); the JSON must be there regardless.
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).expect("doctor --json prints a JSON array");
    let find = |name: &str| rows.iter().find(|r| r["check"] == name);
    for (name, category) in [
        ("lightning_sdk", "vendor:lightning"),
        ("lightning_credentials", "vendor:lightning"),
        ("colab_sdk", "vendor:colab"),
        ("colab_login", "vendor:colab"),
    ] {
        let row = find(name).unwrap_or_else(|| panic!("missing doctor row {name}"));
        assert_eq!(row["category"], category);
    }
    // A bad XRUN_PYTHON falls through to python on PATH, so on a machine that
    // already has the SDKs the rows are OK; when they fail, the detail must
    // carry a message (no crash) that points at the one-command fix.
    for (name, hint) in [
        ("lightning_sdk", "xrun install sdk lightning"),
        ("colab_sdk", "xrun install sdk colab"),
    ] {
        let row = find(name).unwrap();
        if row["status"] == "FAIL" {
            assert!(row["detail"].as_str().unwrap().contains(hint), "{row}");
        }
    }
}

#[test]
fn doctor_without_new_vendors_configured_skips_their_rows() {
    let tmp = TempDir::new().unwrap();
    // A manifest naming no lightning/colab and no creds: the bridge is not spawned.
    let out = xrun(&tmp).args(["doctor", "--json"]).output().unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    // `lightning_sdk` may exist only if this machine really has a native
    // Lightning login, so only assert the rows are consistent pairs.
    let has = |n: &str| rows.iter().any(|r| r["check"] == n);
    assert_eq!(has("lightning_sdk"), has("lightning_credentials"));
    assert_eq!(has("colab_sdk"), has("colab_login"));
}

fn probe_json(tmp: &TempDir, vendor: &str) -> serde_json::Value {
    let out = xrun(tmp)
        .args(["config", "probe", "--vendor", vendor])
        .env_remove("XRUN_PROBE_LIGHTNING_API_KEY")
        .env_remove("XRUN_PROBE_LIGHTNING_USER_ID")
        .output()
        .unwrap();
    assert!(out.status.success(), "probe always exits 0");
    let line = String::from_utf8(out.stdout).unwrap();
    serde_json::from_str(line.trim()).expect("probe prints one JSON object")
}

#[test]
fn probe_lightning_without_creds_is_ok_false() {
    let tmp = TempDir::new().unwrap();
    let v = probe_json(&tmp, "lightning");
    assert_eq!(v["vendor"], "lightning");
    assert_eq!(v["ok"], false);
    assert!(v["detail"].is_string());
}

#[test]
fn config_set_lightning_teamspace_requires_owner_slash_name() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp).args(["config", "init"]).assert().success();
    for bad in ["Gerrux Org", "default-project", "/x", "x/", "a/b/c"] {
        xrun(&tmp)
            .args(["config", "set", "lightning.teamspace", bad])
            .assert()
            .failure()
            .stderr(predicate::str::contains("owner/name"));
    }
    xrun(&tmp)
        .args([
            "config",
            "set",
            "lightning.teamspace",
            "gerrux-org/default-project",
        ])
        .assert()
        .success();
}

#[test]
fn probe_lightning_uses_stored_credentials_without_env() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp).args(["config", "init"]).assert().success();
    for (k, v) in [
        ("lightning.api_key", "test-key-abcdef123456"),
        ("lightning.user_id", "user-42"),
    ] {
        xrun(&tmp).args(["config", "set", k, v]).assert().success();
    }
    // The probe must hand the stored pair to the bridge: whatever the bridge
    // answers (no python, no SDK, bad key), it is never the "no credentials"
    // short-circuit that an env-only probe produced.
    let v = probe_json(&tmp, "lightning");
    assert_eq!(v["ok"], false);
    let detail = v["detail"].as_str().unwrap();
    assert!(
        !detail.contains("no API key and no"),
        "stored credentials ignored: {detail}"
    );
}

#[test]
fn probe_lightning_rejects_half_a_credential_pair() {
    let tmp = TempDir::new().unwrap();
    let out = xrun(&tmp)
        .args(["config", "probe", "--vendor", "lightning"])
        .env("XRUN_PROBE_LIGHTNING_API_KEY", "test-key-abc")
        .env_remove("XRUN_PROBE_LIGHTNING_USER_ID")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["detail"].as_str().unwrap().contains("together"));
}

#[test]
fn probe_colab_always_prints_one_json_object() {
    // An unusable `XRUN_PYTHON` falls back to the PATH python, which on a
    // developer machine may have colab-cli installed and a token on disk, so
    // `ok` is environment-dependent; the contract is the JSON shape and a
    // clean exit, never a crash or a hang.
    let tmp = TempDir::new().unwrap();
    let v = probe_json(&tmp, "colab");
    assert_eq!(v["vendor"], "colab");
    assert!(v["ok"].is_boolean());
    assert!(v["detail"].is_string());
}

#[test]
fn config_lightning_keys_round_trip_and_mask() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp).args(["config", "init"]).assert().success();
    for (k, v) in [
        ("lightning.api_key", "test-key-abcdef123456"),
        ("lightning.user_id", "user-42"),
        ("lightning.teamspace", "me/proj"),
    ] {
        xrun(&tmp)
            .args(["config", "set", k, v])
            .assert()
            .success()
            .stdout(predicate::str::contains(k));
    }
    // Plain show: the api key never appears, the identifiers do.
    xrun(&tmp)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("test-key-abcdef123456").not())
        .stdout(predicate::str::contains("lightning.api_key: <set>"))
        .stdout(predicate::str::contains("lightning.user_id: user-42"))
        .stdout(predicate::str::contains("lightning.teamspace: me/proj"));
    // --secrets: only the tail of the key.
    xrun(&tmp)
        .args(["config", "show", "--secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("test-key-abcdef123456").not())
        .stdout(predicate::str::contains("123456"));
    for k in [
        "lightning.api_key",
        "lightning.user_id",
        "lightning.teamspace",
    ] {
        xrun(&tmp)
            .args(["config", "unset", k])
            .assert()
            .success()
            .stdout(predicate::str::contains("<unset>"));
    }
    xrun(&tmp)
        .args(["config", "show"])
        .assert()
        .stdout(predicate::str::contains("lightning.api_key: <unset>"))
        .stdout(predicate::str::contains("lightning.user_id: <unset>"));
}

#[test]
fn config_login_requires_a_tty_for_colab() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .args(["config", "login", "colab"])
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a TTY"));
}

#[test]
fn config_login_rejects_other_vendors() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .args(["config", "login", "lightning"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("login is only needed for colab"));
}

#[test]
fn init_writes_lightning_credentials_and_requires_the_pair() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .args([
            "init",
            "--non-interactive",
            "--json",
            "--lightning-key",
            "-",
            "--lightning-user-id",
            "user-42",
            "--lightning-teamspace",
            "me/proj",
        ])
        .write_stdin("test-key-abc\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("lightning.api_key"))
        .stdout(predicate::str::contains("lightning.user_id"))
        .stdout(predicate::str::contains("lightning.teamspace"))
        .stdout(predicate::str::contains("test-key-abc").not());

    let tmp2 = TempDir::new().unwrap();
    xrun(&tmp2)
        .args(["init", "--non-interactive", "--lightning-key", "k"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--lightning-key requires --lightning-user-id",
        ));
    xrun(&tmp2)
        .args(["init", "--non-interactive", "--lightning-user-id", "u"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--lightning-user-id requires --lightning-key",
        ));
}

/// A real interpreter on PATH usable as `XRUN_PYTHON` (a bare program, no args).
fn real_python() -> Option<&'static str> {
    ["python", "python3"].into_iter().find(|p| {
        std::process::Command::new(p)
            .args(["-c", "import sys"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

#[test]
fn install_sdk_lightning_dry_run_prints_the_pip_command() {
    let Some(py) = real_python() else {
        eprintln!("skip: no python on PATH");
        return;
    };
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .env("XRUN_PYTHON", py)
        .args(["install", "sdk", "lightning", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("-m pip install lightning-sdk"))
        .stdout(predicate::str::contains("google-colab-cli").not());
}

#[test]
fn install_sdk_all_dry_run_upgrade_lists_both_packages() {
    let Some(py) = real_python() else {
        eprintln!("skip: no python on PATH");
        return;
    };
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .env("XRUN_PYTHON", py)
        .args(["install", "sdk", "all", "--dry-run", "--upgrade"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "-m pip install --upgrade lightning-sdk google-colab-cli",
        ));
}

#[test]
fn install_sdk_rejects_unknown_target() {
    let tmp = TempDir::new().unwrap();
    xrun(&tmp)
        .args(["install", "sdk", "bogus"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("lightning"))
        .stderr(predicate::str::contains("colab"))
        .stderr(predicate::str::contains("all"));
}

#[test]
fn install_sdk_without_python_fails_clearly() {
    let tmp = TempDir::new().unwrap();
    // xrun() points XRUN_PYTHON at a nonexistent binary, but PATH may still
    // have a real python; only assert when discovery really finds nothing.
    let out = xrun(&tmp)
        .args(["install", "sdk", "lightning", "--dry-run"])
        .output()
        .unwrap();
    if !out.status.success() {
        assert!(String::from_utf8_lossy(&out.stderr).contains("python interpreter not found"));
    }
}
