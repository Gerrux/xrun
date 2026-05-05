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
        .args(["init", "--non-interactive", "--kaggle-username", "alice"])
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

// ── Schema-driven `config set` ─────────────────────────────────────────────
//
// The dotted-path setter walks GlobalConfig/Credentials via serde_json. These
// tests exercise paths the old hand-rolled match couldn't reach (budget.*,
// defaults.exp_dir, vendors.<name>.<field>) plus the type-coercion edges.

fn init_dir() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "init"])
        .assert()
        .success();
    dir
}

fn set(dir: &tempfile::TempDir, key: &str, value: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["config", "set", key, value])
        .assert()
}

#[test]
fn config_set_budget_float_field() {
    let dir = init_dir();
    set(&dir, "budget.max_lifetime_hours", "12").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("max_lifetime_hours = 12"),
        "missing budget.max_lifetime_hours:\n{toml}"
    );
}

#[test]
fn config_set_budget_nullable_numeric_via_hint_path() {
    // budget.daily_budget_usd is Option<f64>, defaults to null. Without the
    // NUMERIC_HINT_PATHS table the setter would coerce the input as a string
    // and fail to deserialize back into Option<f64>.
    let dir = init_dir();
    set(&dir, "budget.daily_budget_usd", "50").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("daily_budget_usd = 50"),
        "expected numeric daily_budget_usd:\n{toml}"
    );
}

#[test]
fn config_set_budget_bool_field() {
    let dir = init_dir();
    set(&dir, "budget.daily_budget_hard", "true").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(toml.contains("daily_budget_hard = true"), "{toml}");

    set(&dir, "budget.daily_budget_hard", "off").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(toml.contains("daily_budget_hard = false"), "{toml}");
}

#[test]
fn config_set_rejects_non_numeric_for_numeric_field() {
    let dir = init_dir();
    set(&dir, "budget.max_lifetime_hours", "not-a-number").failure();
}

#[test]
fn config_set_rejects_non_bool_for_bool_field() {
    let dir = init_dir();
    set(&dir, "budget.daily_budget_hard", "maybe").failure();
}

#[test]
fn config_set_defaults_exp_dir() {
    // String field that the old hand-rolled match didn't support.
    let dir = init_dir();
    set(&dir, "defaults.exp_dir", "exp/").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(toml.contains("exp_dir = \"exp/\""), "{toml}");
}

#[test]
fn config_set_defaults_vendor_accepts_all_variants() {
    for v in ["vast", "kaggle", "local", "ssh"] {
        let dir = init_dir();
        set(&dir, "defaults.vendor", v).success();
        let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            toml.contains(&format!("vendor = \"{v}\"")),
            "vendor {v} not stored:\n{toml}"
        );
    }
}

#[test]
fn config_set_defaults_vendor_rejects_unknown_with_listing() {
    let dir = init_dir();
    set(&dir, "defaults.vendor", "runpod")
        .failure()
        .stderr(contains("unknown vendor"))
        .stderr(contains("vast"))
        .stderr(contains("kaggle"));
}

#[test]
fn config_set_vendors_namespace_auto_vivifies() {
    let dir = init_dir();
    set(&dir, "vendors.vast.default_gpu", "RTX_5090").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("[vendors.vast]") && toml.contains("default_gpu = \"RTX_5090\""),
        "{toml}"
    );
}

#[test]
fn config_set_vendors_extra_auto_vivifies() {
    let dir = init_dir();
    set(&dir, "vendors.kaggle.extra.region", "us-east-1").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("[vendors.kaggle.extra]") && toml.contains("region = \"us-east-1\""),
        "{toml}"
    );
}

#[test]
fn config_set_vendors_rejects_unknown_vendor() {
    let dir = init_dir();
    set(&dir, "vendors.runpod.default_gpu", "H100")
        .failure()
        .stderr(contains("unknown vendor"));
}

#[test]
fn config_set_unknown_key_fails() {
    let dir = init_dir();
    set(&dir, "nonsense.key", "x")
        .failure()
        .stderr(contains("unknown config key"));
}

#[test]
fn config_set_credential_via_schema() {
    // vast.api_key now goes through the schema-driven path; verify it lands
    // in credentials.toml the same way the old hand-rolled match did.
    let dir = init_dir();
    set(&dir, "vast.api_key", "test-key-xyz").success();
    let creds = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(creds.contains("api_key = \"test-key-xyz\""), "{creds}");
}

#[test]
fn config_set_metrics_sinks_csv() {
    let dir = init_dir();
    set(&dir, "metrics.sinks", "mlflow, wandb").success();
    let toml = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(
        toml.contains("\"mlflow\"") && toml.contains("\"wandb\""),
        "metrics.sinks not parsed as CSV array:\n{toml}"
    );
}

#[test]
fn init_non_interactive_rejects_unknown_sink() {
    // `wandb` was the v0.5 placeholder — replaced with `comet` (still v0.8)
    // once wandb landed as a real sink in v0.7.
    let dir = tempdir().unwrap();
    Command::cargo_bin("xrun")
        .unwrap()
        .env("XRUN_CONFIG_DIR", dir.path())
        .env("XRUN_DATA_DIR", dir.path().join("data"))
        .args(["init", "--non-interactive", "--sink", "comet"])
        .assert()
        .failure()
        .stderr(contains("unknown sink"));
}
