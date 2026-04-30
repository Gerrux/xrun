use std::path::Path;
use xrun_kaggle::cli::{KaggleCli, KaggleProcess, KernelState, KernelStatus};
use xrun_kaggle::error::KaggleError;

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
