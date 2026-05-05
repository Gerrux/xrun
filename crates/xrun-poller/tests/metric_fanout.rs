/// Integration tests for the metric-sink fan-out (MLflow path) in the
/// poller. Uses wiremock to simulate the MLflow REST API. Slice 3 broadens
/// the file's scope to a multi-sink fan-out, but the MLflow-specific wire-
/// level checks live here because they're easy to exercise in isolation.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;

use tempfile::TempDir;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};
use xrun_core::{
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{RunId, RunStatus, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};
use xrun_poller::{CancellationToken, MetricSinksConfig, MlflowSubConfig, Poller, PollerConfig};

// ---------------------------------------------------------------------------
// Minimal mock vendor that emits a done event after one tick
// ---------------------------------------------------------------------------

struct MockVendor {
    events_queue: RefCell<VecDeque<Vec<u8>>>,
    metrics_queue: RefCell<VecDeque<Vec<u8>>>,
}

impl MockVendor {
    fn with_done_after_metrics(metrics_batches: Vec<Vec<u8>>) -> Self {
        let done_event = b"{\"ts\":\"2024-01-01T00:00:00Z\",\"stage\":\"done\",\"status\":\"ok\",\"msg\":\"done\"}\n".to_vec();
        Self {
            events_queue: RefCell::new(vec![vec![], done_event].into()),
            metrics_queue: RefCell::new(metrics_batches.into()),
        }
    }
}

impl VendorAdapter for MockVendor {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn validate(&self, _: &Manifest) -> Result<(), VendorError> {
        Ok(())
    }
    fn dry_run_plan(&self, _: &Manifest) -> Result<DryRunPlan, VendorError> {
        Err(VendorError::NotImplemented)
    }
    fn provision(&self, _: &Manifest) -> Result<InstanceHandle, VendorError> {
        Err(VendorError::NotImplemented)
    }
    fn upload(&self, _: &InstanceHandle, _: &[DataSource]) -> Result<(), VendorError> {
        Ok(())
    }
    fn execute(&self, _: &InstanceHandle, _: &RunSpec) -> Result<(), VendorError> {
        Ok(())
    }
    fn tail(&self, _h: &InstanceHandle, file: &str, _offset: u64) -> Result<Vec<u8>, VendorError> {
        if file.contains("events") {
            Ok(self
                .events_queue
                .borrow_mut()
                .pop_front()
                .unwrap_or_default())
        } else {
            Ok(self
                .metrics_queue
                .borrow_mut()
                .pop_front()
                .unwrap_or_default())
        }
    }
    fn pull(&self, _: &InstanceHandle, _: &str, _: &Path) -> Result<(), VendorError> {
        Ok(())
    }
    fn destroy(&self, _: &InstanceHandle) -> Result<(), VendorError> {
        Ok(())
    }
}

fn make_store(tmp: &TempDir) -> (Store, RunId) {
    let db_path = tmp.path().join("xrun.db");
    let mut store = Store::open(&db_path).unwrap();
    let run_id = store
        .create_run("test-run", "hash123", "manifest.yaml", "mock", &[])
        .unwrap();
    (store, run_id)
}

fn make_handle() -> InstanceHandle {
    InstanceHandle {
        id: "mock-instance-1".to_string(),
        vendor: "mock".to_string(),
        ssh_host: None,
        ssh_port: None,
        ssh_user: "user".to_string(),
    }
}

/// Test: poller with MLflow configured sends create_run + log_batch + update_run=FINISHED
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_mlflow_mirror_sends_requests() {
    let server = MockServer::start().await;

    // experiment get-by-name → 404
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/experiments/get-by-name"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "error_code": "RESOURCE_DOES_NOT_EXIST"
        })))
        .mount(&server)
        .await;

    // experiment create → 200
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/experiments/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "experiment_id": "exp-1"
        })))
        .mount(&server)
        .await;

    // runs/create → 200
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "run": {
                "info": {
                    "run_id": "mlflow-run-abc",
                    "experiment_id": "exp-1",
                    "status": "RUNNING"
                }
            }
        })))
        .mount(&server)
        .await;

    // log-batch → 200 (for metrics)
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-batch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    // runs/update (FINISHED) → 200
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/update"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "run_info": { "run_id": "mlflow-run-abc", "status": "FINISHED" }
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let (store, run_id) = make_store(&tmp);
    let handle = make_handle();

    // One batch of 2 metrics, then done event
    let metrics_batch = b"{\"step\":1,\"key\":\"loss\",\"value\":0.5,\"ts\":\"2024-01-01T00:00:00Z\"}\n\
                          {\"step\":1,\"key\":\"acc\",\"value\":0.9,\"ts\":\"2024-01-01T00:00:00Z\"}\n"
        .to_vec();
    let vendor = MockVendor::with_done_after_metrics(vec![metrics_batch]);

    let sinks_cfg = MetricSinksConfig {
        experiment: "test-experiment".to_string(),
        run_name: Some("test-run".to_string()),
        vendor: "mock".to_string(),
        instance_id: None,
        mlflow: Some(MlflowSubConfig {
            url: server.uri(),
            auth: None,
            log_args_as_params: false,
        }),
        wandb: None,
    };

    let poller = Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle,
        tmp.path().to_path_buf(),
    )
    .with_metric_sinks(sinks_cfg)
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..PollerConfig::default()
    });

    let cancel = CancellationToken::new();
    let status = poller.run(cancel).unwrap();
    assert_eq!(status, RunStatus::Done);

    // Verify the MLflow requests were received
    let requests = server.received_requests().await.unwrap();

    let create_run_reqs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().contains("runs/create"))
        .collect();
    assert!(
        !create_run_reqs.is_empty(),
        "should have created an MLflow run"
    );

    let update_reqs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().contains("runs/update"))
        .collect();
    assert!(
        !update_reqs.is_empty(),
        "should have updated MLflow run to FINISHED"
    );

    // Verify the update contains FINISHED status
    if let Some(update_req) = update_reqs.first() {
        let body: serde_json::Value = serde_json::from_slice(&update_req.body).unwrap();
        assert_eq!(body["status"], "FINISHED");
    }
}

/// Test: MLflow 500 errors on log_batch do NOT prevent run from completing in SQLite
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_mlflow_degrade_on_log_batch_500() {
    let server = MockServer::start().await;

    // All MLflow calls return 500
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let (store, run_id) = make_store(&tmp);
    let db_path = tmp.path().join("xrun.db");
    let handle = make_handle();

    let done_event = b"{\"ts\":\"2024-01-01T00:00:00Z\",\"stage\":\"done\",\"status\":\"ok\",\"msg\":\"done\"}\n".to_vec();
    let vendor = MockVendor {
        events_queue: RefCell::new(vec![done_event].into()),
        metrics_queue: RefCell::new(VecDeque::new()),
    };

    let sinks_cfg = MetricSinksConfig {
        experiment: "test-experiment".to_string(),
        run_name: None,
        vendor: "mock".to_string(),
        instance_id: None,
        mlflow: Some(MlflowSubConfig {
            url: server.uri(),
            auth: None,
            log_args_as_params: false,
        }),
        wandb: None,
    };

    let poller = Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle,
        tmp.path().to_path_buf(),
    )
    .with_metric_sinks(sinks_cfg)
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..PollerConfig::default()
    });

    let cancel = CancellationToken::new();
    let status = poller.run(cancel).unwrap();

    // Run MUST complete in SQLite despite MLflow failures
    assert_eq!(
        status,
        RunStatus::Done,
        "run should complete even when MLflow is down"
    );

    // Verify in DB that status is Done
    let verify_store = Store::open(&db_path).unwrap();
    let run = verify_store.get_run(&run_id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Done);
}

/// Multi-sink fan-out: both MLflow and WandB receive the same metric batch
/// when both are configured. Catches the regression where `MetricFanOut`
/// would log only to the first opened sink.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multi_sink_fanout_to_mlflow_and_wandb() {
    use xrun_poller::WandbSubConfig;

    // Two independent mock servers so we can count requests per sink.
    let mlflow_server = MockServer::start().await;
    let wandb_server = MockServer::start().await;

    // MLflow happy path
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/experiments/get-by-name"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "experiment": { "experiment_id": "1" }
        })))
        .mount(&mlflow_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "run": { "info": { "run_id": "ml-run-1", "experiment_id": "1", "status": "RUNNING" } }
        })))
        .mount(&mlflow_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-batch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&mlflow_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/update"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&mlflow_server)
        .await;

    // WandB happy path: upsertBucket then file_stream
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "upsertBucket": {
                    "bucket": {
                        "id": "wb-bucket-1",
                        "name": "xrun-fanout",
                        "displayName": "fanout",
                        "project": { "name": "exp", "entity": { "name": "ent" } }
                    }
                }
            }
        })))
        .mount(&wandb_server)
        .await;
    // The wandb run name is `xrun-{run_id}`; we don't pre-know the ulid,
    // so match any file_stream URL under our entity/project.
    Mock::given(method("POST"))
        .and(path_regex(r"^/files/ent/exp/xrun-[A-Z0-9]+/file_stream$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&wandb_server)
        .await;

    let tmp = TempDir::new().unwrap();
    let (store, run_id) = make_store(&tmp);
    let db_path = tmp.path().join("xrun.db");

    let handle = make_handle();

    let metrics_batch =
        b"{\"step\":1,\"key\":\"loss\",\"value\":0.5,\"ts\":\"2024-01-01T00:00:00Z\"}\n".to_vec();
    let vendor = MockVendor::with_done_after_metrics(vec![metrics_batch]);

    let sinks_cfg = MetricSinksConfig {
        experiment: "exp".to_string(),
        run_name: Some("fanout".into()),
        vendor: "mock".to_string(),
        instance_id: None,
        mlflow: Some(MlflowSubConfig {
            url: mlflow_server.uri(),
            auth: None,
            log_args_as_params: false,
        }),
        wandb: Some(WandbSubConfig {
            api_key: "test-key".into(),
            entity: Some("ent".into()),
            api_base: Some(wandb_server.uri()),
            web_base: Some(wandb_server.uri()),
        }),
    };

    let poller = Poller::new(
        run_id.clone(),
        store,
        Box::new(vendor),
        handle,
        tmp.path().to_path_buf(),
    )
    .with_metric_sinks(sinks_cfg)
    .with_config(PollerConfig {
        interval_active_secs: 0,
        interval_idle_secs: 0,
        ..PollerConfig::default()
    });

    let cancel = CancellationToken::new();
    let status = poller.run(cancel).unwrap();
    assert_eq!(status, RunStatus::Done);

    // MLflow: at least 1 log-batch call
    let ml_reqs = mlflow_server.received_requests().await.unwrap();
    let ml_log_batches: Vec<_> = ml_reqs
        .iter()
        .filter(|r| r.url.path().contains("log-batch"))
        .collect();
    assert!(
        !ml_log_batches.is_empty(),
        "MLflow should have received at least one log-batch"
    );

    // WandB: at least 1 file_stream call carrying the metric. (The exact
    // path includes the run name which is generated from the ulid; we
    // just check that some file_stream POST happened.)
    let wb_reqs = wandb_server.received_requests().await.unwrap();
    let wb_streams: Vec<_> = wb_reqs
        .iter()
        .filter(|r| r.url.path().contains("file_stream"))
        .collect();
    assert!(
        !wb_streams.is_empty(),
        "WandB should have received at least one file_stream POST"
    );

    // Verify SQLite still authoritative
    let verify_store = Store::open(&db_path).unwrap();
    let run = verify_store.get_run(&run_id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Done);
}
