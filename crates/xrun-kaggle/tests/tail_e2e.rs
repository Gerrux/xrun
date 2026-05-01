//! End-to-end test for the kaggle adapter's live `tail()` against a mock
//! MLflow server. Covers the full path: tag-based run lookup →
//! artifacts/list → chunk download → byte slice past offset.
//!
//! The unit tests in `log_stream::tests` exercise the pure reassembly logic;
//! these tests verify the HTTP wiring, sort order, and the contract the
//! poller depends on (offset semantics, empty when no chunks yet, …).

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use xrun_core::store::RunId;
use xrun_core::vendor::{InstanceHandle, VendorAdapter};
use xrun_kaggle::cli::KaggleProcess;
use xrun_kaggle::error::KaggleError;
use xrun_kaggle::KaggleAdapter;

/// Build an adapter + matching RunId so the mock filter and embedded ID
/// stay in sync regardless of ULID randomness.
fn adapter_with_run(mlflow_url: &str) -> (KaggleAdapter, RunId) {
    let adapter = KaggleAdapter::with_process(Box::new(PanicKaggleProcess))
        .with_mlflow(mlflow_url.to_string(), None);
    let run_id = RunId::new();
    adapter.set_run_id(&run_id);
    (adapter, run_id)
}

const STDOUT_FILE: &str = "/kaggle/working/stdout.log";

fn handle() -> InstanceHandle {
    InstanceHandle {
        id: "kaggle:alice/k1".to_string(),
        vendor: "kaggle".to_string(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "kaggle".to_string(),
    }
}

/// Stub kaggle CLI — never actually invoked by `tail()` so it just panics on
/// any call, surfacing accidental couplings between tail and the kaggle CLI.
struct PanicKaggleProcess;
impl KaggleProcess for PanicKaggleProcess {
    fn push(&self, _: &std::path::Path) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle push")
    }
    fn status(&self, _: &str) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle status")
    }
    fn output(&self, _: &str, _: &std::path::Path) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle output")
    }
    fn cancel(&self, _: &str) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle cancel")
    }
    fn list_mine(&self) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle list_mine")
    }
    fn config_view(&self) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle config_view")
    }
    fn datasets_status(&self, _: &str) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle datasets_status")
    }
    fn datasets_create(&self, _: &std::path::Path) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle datasets_create")
    }
    fn datasets_version(
        &self,
        _: &std::path::Path,
        _: &str,
    ) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle datasets_version")
    }
    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        panic!("tail() must not call kaggle datasets_list_mine")
    }
}

async fn mount_search_runs(server: &MockServer, mlflow_run_id: &str, xrun_run_id: &RunId) {
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/search"))
        .and(body_partial_json(json!({
            "filter": format!("tags.xrun_run_id = '{}'", xrun_run_id),
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "runs": [{"info": {"run_id": mlflow_run_id, "experiment_id": "1"}}]
        })))
        .mount(server)
        .await;
}

/// `runs/get` is needed in the new flow to resolve the run's `artifact_uri`.
/// The proxy treats `?run_id=` as advisory only, so the adapter must dial
/// the storage path directly.
async fn mount_runs_get(server: &MockServer, mlflow_run_id: &str) {
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/runs/get"))
        .and(query_param("run_id", mlflow_run_id))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "run": {"info": {
                "run_id": mlflow_run_id,
                "artifact_uri": format!("mlflow-artifacts:/1/{}/artifacts", mlflow_run_id),
            }}
        })))
        .mount(server)
        .await;
}

async fn mount_get_experiment(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/experiments/get-by-name"))
        .and(query_param("experiment_name", "xrun-logs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "experiment": {"experiment_id": "1", "name": "xrun-logs"}
        })))
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_returns_empty_when_no_mlflow() {
    let adapter = KaggleAdapter::with_process(Box::new(PanicKaggleProcess));
    let bytes = adapter
        .tail(&handle(), STDOUT_FILE, 0)
        .expect("tail without MLflow must not error");
    assert!(bytes.is_empty(), "no MLflow → empty");
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_returns_empty_for_non_stdout_files() {
    let server = MockServer::start().await;
    let (adapter, _run_id) = adapter_with_run(&server.uri());
    // Even with MLflow configured, events/metrics still go through the
    // post-completion ingest path; tail only handles stdout.
    let bytes = adapter
        .tail(&handle(), "/workspace/run/events.jsonl", 0)
        .expect("tail must not error");
    assert!(bytes.is_empty());
    let received = server.received_requests().await.unwrap_or_default();
    assert!(received.is_empty(), "non-stdout file must not hit MLflow");
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_returns_empty_when_streamer_run_not_found_yet() {
    let server = MockServer::start().await;
    mount_get_experiment(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"runs": []})))
        .mount(&server)
        .await;

    let (adapter, _run_id) = adapter_with_run(&server.uri());
    let bytes = adapter
        .tail(&handle(), STDOUT_FILE, 0)
        .expect("missing streamer run is non-fatal");
    assert!(
        bytes.is_empty(),
        "kernel still warming up → empty, retry next tick"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_concatenates_chunks_from_offset_zero() {
    let server = MockServer::start().await;
    mount_get_experiment(&server).await;
    let (adapter, run_id) = adapter_with_run(&server.uri());
    mount_search_runs(&server, "mlflow-run-7", &run_id).await;
    mount_runs_get(&server, "mlflow-run-7").await;

    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts"))
        .and(query_param("path", "1/mlflow-run-7/artifacts/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "files": [
                {"path": "log_000002.txt", "is_dir": false, "file_size": 5},
                {"path": "log_000001.txt", "is_dir": false, "file_size": 6},
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts/1/mlflow-run-7/artifacts/logs/log_000001.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello\n".to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts/1/mlflow-run-7/artifacts/logs/log_000002.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"world".to_vec()))
        .mount(&server)
        .await;

    let bytes = adapter.tail(&handle(), STDOUT_FILE, 0).expect("tail ok");
    assert_eq!(
        bytes, b"hello\nworld",
        "chunks must reassemble in seq order regardless of listing order"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_skips_chunks_before_offset() {
    let server = MockServer::start().await;
    mount_get_experiment(&server).await;
    let (adapter, run_id) = adapter_with_run(&server.uri());
    mount_search_runs(&server, "mlflow-run-7", &run_id).await;
    mount_runs_get(&server, "mlflow-run-7").await;

    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "files": [
                {"path": "log_000001.txt", "is_dir": false, "file_size": 6},
                {"path": "log_000002.txt", "is_dir": false, "file_size": 5},
            ]
        })))
        .mount(&server)
        .await;

    let downloaded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dl_clone = downloaded.clone();
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts/1/mlflow-run-7/artifacts/logs/log_000001.txt"))
        .respond_with(move |_: &wiremock::Request| {
            dl_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_bytes(b"hello\n".to_vec())
        })
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts/1/mlflow-run-7/artifacts/logs/log_000002.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"world".to_vec()))
        .mount(&server)
        .await;

    let bytes = adapter
        .tail(&handle(), STDOUT_FILE, 6) // past chunk 1
        .expect("tail ok");
    assert_eq!(bytes, b"world");
    assert!(
        !downloaded.load(std::sync::atomic::Ordering::SeqCst),
        "tail must skip chunks fully covered by offset (avoids redundant network)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_returns_empty_when_offset_at_total_size() {
    let server = MockServer::start().await;
    mount_get_experiment(&server).await;
    let (adapter, run_id) = adapter_with_run(&server.uri());
    mount_search_runs(&server, "mlflow-run-7", &run_id).await;
    mount_runs_get(&server, "mlflow-run-7").await;

    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "files": [
                {"path": "log_000001.txt", "is_dir": false, "file_size": 11}
            ]
        })))
        .mount(&server)
        .await;

    let bytes = adapter
        .tail(&handle(), STDOUT_FILE, 11)
        .expect("offset at EOF must be Ok(empty)");
    assert!(bytes.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn tail_handles_offset_inside_chunk() {
    let server = MockServer::start().await;
    mount_get_experiment(&server).await;
    let (adapter, run_id) = adapter_with_run(&server.uri());
    mount_search_runs(&server, "mlflow-run-7", &run_id).await;
    mount_runs_get(&server, "mlflow-run-7").await;

    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "files": [
                {"path": "log_000001.txt", "is_dir": false, "file_size": 10},
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts/1/mlflow-run-7/artifacts/logs/log_000001.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"0123456789".to_vec()))
        .mount(&server)
        .await;

    let bytes = adapter.tail(&handle(), STDOUT_FILE, 4).expect("tail ok");
    assert_eq!(bytes, b"456789");
}
