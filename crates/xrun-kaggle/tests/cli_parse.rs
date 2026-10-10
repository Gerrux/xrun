use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use xrun_kaggle::cli::{KaggleCli, KaggleProcess, KernelState, KernelStatus};
use xrun_kaggle::error::KaggleError;
use xrun_kaggle::KaggleAdapter;

/// `datasets status` mock: records the slugs it was asked about and answers
/// with a canned result.
struct MockDatasetStatus {
    reply: Result<String, ()>,
    asked: Arc<Mutex<Vec<String>>>,
}

impl MockDatasetStatus {
    fn ready() -> Self {
        Self {
            reply: Ok("user/ds has status: ready\n".into()),
            asked: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn forbidden() -> Self {
        Self {
            reply: Err(()),
            asked: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl KaggleProcess for MockDatasetStatus {
    fn push(&self, _dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn output(&self, _slug: &str, _into: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn cancel(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn config_view(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_status(&self, slug: &str) -> Result<String, KaggleError> {
        self.asked.lock().unwrap().push(slug.to_string());
        match &self.reply {
            Ok(s) => Ok(s.clone()),
            Err(()) => Err(KaggleError::CliFailure {
                exit_code: 1,
                stderr: "403 Client Error: Forbidden for url: .../GetDatasetStatus".into(),
            }),
        }
    }
    fn datasets_create(&self, _local_dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_version(&self, _local_dir: &Path, _message: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
}

#[test]
fn wait_datasets_ready_passes_when_ready() {
    let adapter = KaggleAdapter::with_process(Box::new(MockDatasetStatus::ready()));
    let res = adapter.wait_datasets_ready(&["user/ds".to_string()], Duration::from_secs(1));
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn wait_datasets_ready_fails_when_status_call_errors() {
    // The powerline-seg-v1 incident: doctor said FAIL kaggle_dataset, launch
    // logged a warning and pushed the kernel anyway. A status error must
    // stop the launch, with a hint that names the slug and the fix.
    let adapter = KaggleAdapter::with_process(Box::new(MockDatasetStatus::forbidden()));
    let err = adapter
        .wait_datasets_ready(&["user/missing".to_string()], Duration::from_secs(1))
        .expect_err("status error must be fatal");
    let msg = err.to_string();
    assert!(msg.contains("user/missing"), "{msg}");
    assert!(msg.contains("xrun dataset push"), "{msg}");
    assert!(msg.contains("403"), "{msg}");
}

#[test]
fn wait_datasets_ready_strips_version_suffix_for_status() {
    // A user-pinned `owner/name/7` is queried as `owner/name`: that is the
    // only form `kaggle datasets status` accepts.
    let mock = MockDatasetStatus::ready();
    let asked = mock.asked.clone();
    let adapter = KaggleAdapter::with_process(Box::new(mock));
    adapter
        .wait_datasets_ready(
            &["user/ds/7".to_string(), "other/plain".to_string()],
            Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(
        *asked.lock().unwrap(),
        vec!["user/ds".to_string(), "other/plain".to_string()]
    );
}

/// `datasets status` mock for the push-time wait: the first `fail_first`
/// calls fail the way a not-yet-indexed new dataset does (`kaggle CLI failed
/// (exit 1)`), every later call reports `ready`.
struct MockDatasetIndexing {
    fail_first: u32,
    calls: Arc<Mutex<u32>>,
}

impl MockDatasetIndexing {
    fn new(fail_first: u32) -> Self {
        Self {
            fail_first,
            calls: Arc::new(Mutex::new(0)),
        }
    }
}

impl KaggleProcess for MockDatasetIndexing {
    fn push(&self, _dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn output(&self, _slug: &str, _into: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn cancel(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn config_view(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_status(&self, _slug: &str) -> Result<String, KaggleError> {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        if *n <= self.fail_first {
            return Err(KaggleError::CliFailure {
                exit_code: 1,
                stderr: "404 Client Error: Not Found for url: .../GetDatasetStatus".into(),
            });
        }
        Ok("user/ds has status: ready\n".into())
    }
    fn datasets_create(&self, _local_dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_version(&self, _local_dir: &Path, _message: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
}

#[test]
fn wait_dataset_ready_retries_status_errors_until_ready() {
    // arboforge-itd-test, 2026-10-10: right after the first push of a new
    // slug `datasets status` failed, the wait gave up on the first error and
    // verification was skipped with exit 0. A minute later it was `ready`.
    let mock = MockDatasetIndexing::new(2);
    let calls = mock.calls.clone();
    let cli = KaggleCli::with_process(Box::new(mock));
    let res = cli.wait_dataset_ready("user/ds", Duration::from_secs(5), Duration::ZERO);
    assert!(res.is_ok(), "{res:?}");
    assert_eq!(*calls.lock().unwrap(), 3, "two failures, then ready");
}

#[test]
fn wait_dataset_ready_fails_when_readiness_never_known() {
    // Status never succeeds before the timeout: that is an error, not a
    // silent "verification skipped".
    let mock = MockDatasetIndexing::new(u32::MAX);
    let calls = mock.calls.clone();
    let cli = KaggleCli::with_process(Box::new(mock));
    let err = cli
        .wait_dataset_ready("user/ds", Duration::from_millis(300), Duration::ZERO)
        .expect_err("unknown readiness must be an error");
    let msg = err.to_string();
    assert!(msg.contains("user/ds"), "{msg}");
    assert!(msg.contains("readiness unknown"), "{msg}");
    assert!(msg.contains("404"), "{msg}");
    assert!(msg.contains("xrun dataset status user/ds"), "{msg}");
    assert!(*calls.lock().unwrap() > 1, "must retry before giving up");
}

#[test]
fn wait_dataset_ready_times_out_when_never_ready() {
    let cli = KaggleCli::with_process(Box::new(MockDatasetStatus {
        reply: Ok("user/ds has status: pending\n".into()),
        asked: Arc::new(Mutex::new(Vec::new())),
    }));
    let err = cli
        .wait_dataset_ready("user/ds", Duration::from_millis(50), Duration::ZERO)
        .expect_err("never ready must be an error");
    let msg = err.to_string();
    assert!(msg.contains("not ready"), "{msg}");
    assert!(msg.contains("user/ds"), "{msg}");
}

/// Fixture-based status deserialization test
#[test]
fn test_deserialize_complete_status() {
    let json = include_str!("data/kernels_status.json");
    let status: KernelStatus = serde_json::from_str(json).expect("should parse");
    assert_eq!(status.status, KernelState::Complete);
    assert!(status.error_message.is_none());
    assert_eq!(status.run_seconds, Some(42));
}

#[test]
fn test_deserialize_running_status() {
    let json = r#"{"status":"Running","totalRunningTimeSec":15}"#;
    let status: KernelStatus = serde_json::from_str(json).expect("should parse");
    assert_eq!(status.status, KernelState::Running);
    assert_eq!(status.run_seconds, Some(15));
}

#[test]
fn test_deserialize_queued_status() {
    let json = r#"{"status":"Queued"}"#;
    let status: KernelStatus = serde_json::from_str(json).expect("should parse");
    assert_eq!(status.status, KernelState::Queued);
    assert!(status.run_seconds.is_none());
}

#[test]
fn test_deserialize_error_status_with_message() {
    let json = r#"{"status":"Error","failureMessage":"Out of memory"}"#;
    let status: KernelStatus = serde_json::from_str(json).expect("should parse");
    assert_eq!(status.status, KernelState::Error);
    assert_eq!(status.error_message.as_deref(), Some("Out of memory"));
}

#[test]
fn test_deserialize_ignores_unknown_fields() {
    let json = r#"{
        "status": "Complete",
        "totalRunningTimeSec": 60,
        "unknownFutureField": "ignored",
        "anotherNew": 42
    }"#;
    let status: KernelStatus =
        serde_json::from_str(json).expect("should parse with unknown fields");
    assert_eq!(status.status, KernelState::Complete);
    assert_eq!(status.run_seconds, Some(60));
}

// --- Mock process for testing CLI parsing ---

struct MockPushProcess {
    stdout: String,
}

impl KaggleProcess for MockPushProcess {
    fn push(&self, _dir: &Path) -> Result<String, KaggleError> {
        Ok(self.stdout.clone())
    }
    fn status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn output(&self, _slug: &str, _into: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn cancel(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn config_view(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_create(&self, _local_dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_version(&self, _local_dir: &Path, _message: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
}

#[test]
fn test_parse_push_slug_standard() {
    let mock = Box::new(MockPushProcess {
        stdout: "Kernel pushed: myuser/my-experiment\n".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let slug = cli.push(Path::new(".")).expect("should parse slug");
    assert_eq!(slug, "myuser/my-experiment");
}

#[test]
fn test_parse_push_slug_already_exists() {
    let mock = Box::new(MockPushProcess {
        stdout: "Kernel already exists, new version pushed: myuser/existing-kernel\n".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let slug = cli
        .push(Path::new("."))
        .expect("should parse slug from already-exists output");
    assert_eq!(slug, "myuser/existing-kernel");
}

#[test]
fn test_parse_push_slug_fails_on_unrecognized_output() {
    let mock = Box::new(MockPushProcess {
        stdout: "Something unexpected happened".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let result = cli.push(Path::new("."));
    assert!(result.is_err(), "should fail when output is unrecognized");
}

struct MockStatusProcess {
    status_json: String,
}

impl KaggleProcess for MockStatusProcess {
    fn push(&self, _dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn status(&self, _slug: &str) -> Result<String, KaggleError> {
        Ok(self.status_json.clone())
    }
    fn output(&self, _slug: &str, _into: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn cancel(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn config_view(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_create(&self, _local_dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_version(&self, _local_dir: &Path, _message: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
}

#[test]
fn test_cli_status_parses_complete() {
    let mock = Box::new(MockStatusProcess {
        status_json: r#"{"status":"Complete","totalRunningTimeSec":42}"#.to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should parse");
    assert_eq!(status.status, KernelState::Complete);
    assert_eq!(status.run_seconds, Some(42));
}

#[test]
fn test_cli_status_parses_error() {
    let mock = Box::new(MockStatusProcess {
        status_json: r#"{"status":"Error","failureMessage":"GPU OOM"}"#.to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should parse");
    assert_eq!(status.status, KernelState::Error);
    assert_eq!(status.error_message.as_deref(), Some("GPU OOM"));
}

// Kaggle CLI 1.8.x dropped JSON-mode for `kernels status` — only plain text
// with the `KernelWorkerStatus.<STATE>` enum is available now. The cli
// parser must keep working against both formats.

#[test]
fn test_cli_status_parses_kernelworkerstatus_complete() {
    let mock = Box::new(MockStatusProcess {
        status_json: "user/slug has status \"KernelWorkerStatus.COMPLETE\"\n".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should parse text format");
    assert_eq!(status.status, KernelState::Complete);
    assert!(status.error_message.is_none());
}

#[test]
fn test_cli_status_parses_kernelworkerstatus_running() {
    let mock = Box::new(MockStatusProcess {
        status_json: "user/slug has status \"KernelWorkerStatus.RUNNING\"\n".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should parse text format");
    assert_eq!(status.status, KernelState::Running);
}

#[test]
fn test_cli_status_parses_kernelworkerstatus_queued() {
    let mock = Box::new(MockStatusProcess {
        status_json: "user/slug has status \"KernelWorkerStatus.QUEUED\"\n".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should parse text format");
    assert_eq!(status.status, KernelState::Queued);
}

#[test]
fn test_cli_status_parses_kernelworkerstatus_error_with_failure_message() {
    let mock = Box::new(MockStatusProcess {
        status_json: "user/slug has status \"KernelWorkerStatus.ERROR\"\n\
                      Failure message: \"Your notebook tried to allocate \
                      more memory than is available.\"\n"
            .to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should parse text format");
    assert_eq!(status.status, KernelState::Error);
    assert_eq!(
        status.error_message.as_deref(),
        Some("Your notebook tried to allocate more memory than is available.")
    );
}

#[test]
fn test_cli_status_parses_with_outdated_warning_prefix() {
    // The kaggle CLI prints a self-update warning above the actual status
    // when an upgrade is available — must not confuse the parser.
    let mock = Box::new(MockStatusProcess {
        status_json: "Warning: Looks like you're using an outdated `kaggle` \
                      version (installed: x), please consider upgrading...\n\
                      user/slug has status \"KernelWorkerStatus.COMPLETE\"\n"
            .to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli
        .status("user/slug")
        .expect("should ignore warning prefix");
    assert_eq!(status.status, KernelState::Complete);
}

// --- config view / username parsing ---

struct MockConfigViewProcess {
    stdout: String,
}

impl KaggleProcess for MockConfigViewProcess {
    fn push(&self, _dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn output(&self, _slug: &str, _into: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn cancel(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn config_view(&self) -> Result<String, KaggleError> {
        Ok(self.stdout.clone())
    }
    fn datasets_status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_create(&self, _local_dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_version(&self, _local_dir: &Path, _message: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
}

#[test]
fn test_username_parses_plain_format() {
    let mock = Box::new(MockConfigViewProcess {
        stdout: "username: kartaviychert\nkey: ****\n".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    assert_eq!(cli.username().unwrap(), "kartaviychert");
}

#[test]
fn test_username_parses_with_outdated_warning_prefix() {
    // Real-world breakage: the version-banner caused the old parser to fail
    // even though the config below it was fine.
    let mock = Box::new(MockConfigViewProcess {
        stdout: "Warning: Looks like you're using an outdated `kaggle` \
                 version, please consider upgrading.\n\
                 - username: kartaviychert\n\
                 - key: ****\n"
            .to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    assert_eq!(cli.username().unwrap(), "kartaviychert");
}

#[test]
fn test_username_parses_indented_yaml_style() {
    let mock = Box::new(MockConfigViewProcess {
        stdout: "  username: kartaviychert\n  key: ****\n".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    assert_eq!(cli.username().unwrap(), "kartaviychert");
}

#[test]
fn test_username_parses_json_format() {
    let mock = Box::new(MockConfigViewProcess {
        stdout: r#"{"username": "kartaviychert", "key": "****"}"#.to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    assert_eq!(cli.username().unwrap(), "kartaviychert");
}

// --- dataset_push retry on transient failures -----------------------------

struct MockDatasetRetry {
    create_calls: std::sync::Arc<std::sync::Mutex<u32>>,
    version_calls: std::sync::Arc<std::sync::Mutex<u32>>,
    /// Number of leading attempts that should fail with a transient error.
    create_fail_count: u32,
    /// What the create result should be on the success attempt.
    create_succeeds: bool,
    /// Stderr returned for the "already exists" branch (triggers version path).
    create_already_stderr: Option<String>,
    /// Number of leading version attempts that should fail with a transient error.
    version_fail_count: u32,
}

impl MockDatasetRetry {
    fn new(create_fail_count: u32) -> Self {
        Self {
            create_calls: std::sync::Arc::new(std::sync::Mutex::new(0)),
            version_calls: std::sync::Arc::new(std::sync::Mutex::new(0)),
            create_fail_count,
            create_succeeds: true,
            create_already_stderr: None,
            version_fail_count: 0,
        }
    }
}

impl KaggleProcess for MockDatasetRetry {
    fn push(&self, _dir: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn output(&self, _slug: &str, _into: &Path) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn cancel(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn config_view(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_status(&self, _slug: &str) -> Result<String, KaggleError> {
        unimplemented!()
    }
    fn datasets_create(&self, _local_dir: &Path) -> Result<String, KaggleError> {
        let mut n = self.create_calls.lock().unwrap();
        *n += 1;
        if *n <= self.create_fail_count {
            return Err(KaggleError::CliFailure {
                exit_code: 1,
                stderr: "stdout: \nstderr: HTTP 503 Service Temporarily Unavailable".to_string(),
            });
        }
        if let Some(s) = &self.create_already_stderr {
            return Err(KaggleError::CliFailure {
                exit_code: 1,
                stderr: s.clone(),
            });
        }
        if self.create_succeeds {
            Ok(String::new())
        } else {
            Err(KaggleError::CliFailure {
                exit_code: 2,
                stderr: "stderr: forbidden".to_string(),
            })
        }
    }
    fn datasets_version(&self, _local_dir: &Path, _message: &str) -> Result<String, KaggleError> {
        let mut n = self.version_calls.lock().unwrap();
        *n += 1;
        if *n <= self.version_fail_count {
            return Err(KaggleError::CliFailure {
                exit_code: 1,
                stderr: "stderr: connection reset by peer during commit".to_string(),
            });
        }
        Ok(String::new())
    }
    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        unimplemented!()
    }
}

#[test]
fn test_dataset_push_retries_transient_create_failure() {
    // Two transient 503s, then success — should not bubble up an error.
    let mock = Box::new(MockDatasetRetry::new(2));
    let calls = mock.create_calls.clone();
    let cli = KaggleCli::with_process(mock);

    // Use a unique tempdir so ensure_dataset_metadata writes don't collide.
    let tmp = tempfile::tempdir().unwrap();
    // Force shorter retry cap + zero backoff so the test stays fast.
    std::env::set_var("XRUN_KAGGLE_DATASET_RETRIES", "3");
    std::env::set_var("XRUN_KAGGLE_DATASET_BACKOFF_BASE_SECS", "0");
    let res = cli.dataset_push(tmp.path(), "user/slug", Some("msg"));
    std::env::remove_var("XRUN_KAGGLE_DATASET_RETRIES");
    std::env::remove_var("XRUN_KAGGLE_DATASET_BACKOFF_BASE_SECS");

    assert!(res.is_ok(), "expected retry to recover, got {res:?}");
    assert_eq!(*calls.lock().unwrap(), 3, "expected 3 create attempts");
}

#[test]
fn test_dataset_push_does_not_retry_permanent_failure() {
    // Permanent error (forbidden) — should fail fast on attempt #1.
    let mut mock = MockDatasetRetry::new(0);
    mock.create_succeeds = false;
    let mock = Box::new(mock);
    let calls = mock.create_calls.clone();
    let cli = KaggleCli::with_process(mock);

    let tmp = tempfile::tempdir().unwrap();
    let res = cli.dataset_push(tmp.path(), "user/slug", None);

    assert!(res.is_err());
    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "must not retry on permanent error"
    );
}

#[test]
fn test_cli_status_parses_cancel_acknowledged_as_error() {
    let mock = Box::new(MockStatusProcess {
        status_json: "user/slug has status \"KernelWorkerStatus.CANCEL_ACKNOWLEDGED\"".to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should parse text format");
    assert_eq!(status.status, KernelState::Error);
}

// kaggle CLI 1.8.3 ships a literal-template warning to stdout (the f-string
// was never substituted — kaggle bug). Without filtering, this leaks into
// our JSON parsers and shows up as a confusing `Expecting value: line 1
// column 1 (char 0)`.

#[test]
fn test_push_slug_strips_literal_template_warning() {
    let mock = Box::new(MockPushProcess {
        stdout: "Warning: Looks like you're using an outdated `kaggle`` version \
                 (installed: {current_version}), please consider upgrading to the \
                 latest version ({latest_version_str})\n\
                 Kernel pushed: myuser/k1\n"
            .to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let slug = cli
        .push(Path::new("."))
        .expect("should ignore literal-template warning");
    assert_eq!(slug, "myuser/k1");
}

#[test]
fn test_status_strips_literal_template_warning() {
    // The 1.8.3 variant where curly-brace placeholders weren't substituted.
    let mock = Box::new(MockStatusProcess {
        status_json: "Warning: Looks like you're using an outdated `kaggle`` version \
                      (installed: {current_version}), please consider upgrading...\n\
                      user/slug has status \"KernelWorkerStatus.RUNNING\"\n"
            .to_string(),
    });
    let cli = KaggleCli::with_process(mock);
    let status = cli.status("user/slug").expect("should ignore the warning");
    assert_eq!(status.status, KernelState::Running);
}
