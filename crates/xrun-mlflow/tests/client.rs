use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use xrun_mlflow::{Auth, MlflowClient, MlflowMetric, MlflowParam, MlflowTag};

fn make_client(base_url: &str) -> MlflowClient {
    MlflowClient::new(base_url, None)
}

fn make_client_with_auth(base_url: &str, auth: Auth) -> MlflowClient {
    MlflowClient::new(base_url, Some(auth))
}

/// Test: GET 404 on get-by-name → POST create → returns experiment_id
#[tokio::test]
async fn test_get_or_create_experiment_creates_when_not_found() {
    let server = MockServer::start().await;

    // First call: GET returns 404
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/experiments/get-by-name"))
        .and(query_param("experiment_name", "my-experiment"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "error_code": "RESOURCE_DOES_NOT_EXIST",
            "message": "Could not find experiment with name my-experiment"
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Second call: POST create returns new id
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/experiments/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "experiment_id": "42"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let exp_id = client
        .get_or_create_experiment("my-experiment")
        .await
        .expect("should succeed");

    assert_eq!(exp_id, "42");
    server.verify().await;
}

/// Test: GET 200 returns existing experiment id (no POST)
#[tokio::test]
async fn test_get_or_create_experiment_returns_existing() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/experiments/get-by-name"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "experiment": {
                "experiment_id": "7",
                "name": "existing-exp"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let exp_id = client
        .get_or_create_experiment("existing-exp")
        .await
        .expect("should succeed");

    assert_eq!(exp_id, "7");
    server.verify().await;
}

/// Test: log_batch sends correct JSON body with 5 metrics
#[tokio::test]
async fn test_log_batch_sends_correct_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-batch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let metrics: Vec<MlflowMetric> = (0..5)
        .map(|i| MlflowMetric {
            key: format!("loss_{i}"),
            value: 0.5 - i as f64 * 0.05,
            timestamp: 1700000000000 + i * 1000,
            step: i,
        })
        .collect();

    client
        .log_batch("run-abc", &[], &metrics, &[])
        .await
        .expect("log_batch should succeed");

    // Verify request body was correct by checking received requests
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "should have exactly 1 request");
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(body["run_id"], "run-abc");
    let metrics_arr = body["metrics"].as_array().unwrap();
    assert_eq!(metrics_arr.len(), 5, "should have 5 metrics");
}

/// Test: 401 → Error::Auth
#[tokio::test]
async fn test_401_returns_auth_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/experiments/get-by-name"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error_code": "PERMISSION_DENIED",
            "message": "Unauthorized"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let err = client
        .get_or_create_experiment("secret-exp")
        .await
        .expect_err("should fail with auth error");

    assert!(
        matches!(err, xrun_mlflow::MlflowError::Auth),
        "expected Auth error, got: {err:?}"
    );
}

/// Test: 503 + retry — first 503, second 200 → returns Ok
#[tokio::test]
async fn test_503_retry_succeeds_on_second_attempt() {
    let server = MockServer::start().await;

    // First call: 503
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-batch"))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error_code": "SERVICE_UNAVAILABLE",
            "message": "Temporarily unavailable"
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // Second call: 200
    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-batch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    client
        .log_batch("run-xyz", &[], &[], &[])
        .await
        .expect("should succeed on retry");
}

/// Test: bearer auth header is sent
#[tokio::test]
async fn test_bearer_auth_is_sent() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-batch"))
        .and(wiremock::matchers::header(
            "Authorization",
            "Bearer test-token-123",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client_with_auth(&server.uri(), Auth::Bearer("test-token-123".to_string()));
    client
        .log_batch("run-abc", &[], &[], &[])
        .await
        .expect("should succeed with bearer auth");

    server.verify().await;
}

/// Test: create_run returns run_id
#[tokio::test]
async fn test_create_run_returns_run_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "run": {
                "info": {
                    "run_id": "run-12345",
                    "experiment_id": "1",
                    "status": "RUNNING"
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tags = vec![MlflowTag {
        key: "vendor".to_string(),
        value: "vast".to_string(),
    }];

    let client = make_client(&server.uri());
    let run_id = client
        .create_run("1", &tags)
        .await
        .expect("create_run should succeed");

    assert_eq!(run_id, "run-12345");
    server.verify().await;
}

/// Test: update_run sends FINISHED status
#[tokio::test]
async fn test_update_run_finished() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/update"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "run_info": {
                "run_id": "run-abc",
                "status": "FINISHED"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    client
        .update_run(
            "run-abc",
            xrun_mlflow::RunStatus::Finished,
            Some(1700000000000),
        )
        .await
        .expect("update_run should succeed");

    server.verify().await;
}

/// Test: log_param sends correct body
#[tokio::test]
async fn test_log_param() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-parameter"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    client
        .log_param("run-abc", "learning_rate", "0.001")
        .await
        .expect("log_param should succeed");

    server.verify().await;
}

/// Test: search_runs_by_tag returns run_ids in order
#[tokio::test]
async fn test_search_runs_by_tag() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "runs": [
                {"info": {"run_id": "rA", "experiment_id": "1"}},
                {"info": {"run_id": "rB", "experiment_id": "1"}},
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let ids = client
        .search_runs_by_tag("1", "xrun_run_id", "abc-123")
        .await
        .expect("search_runs_by_tag should succeed");

    assert_eq!(ids, vec!["rA".to_string(), "rB".to_string()]);

    let recv = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&recv[0].body).unwrap();
    assert_eq!(body["experiment_ids"], serde_json::json!(["1"]));
    assert_eq!(body["filter"], "tags.xrun_run_id = 'abc-123'");
}

/// Test: search_runs_by_tag returns empty vec when no matches
#[tokio::test]
async fn test_search_runs_by_tag_empty() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let ids = client
        .search_runs_by_tag("1", "k", "v")
        .await
        .expect("should succeed even when runs key absent");
    assert!(ids.is_empty());
}

/// Test: get_run_artifact_path strips `mlflow-artifacts:` scheme prefix
#[tokio::test]
async fn test_get_run_artifact_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow/runs/get"))
        .and(query_param("run_id", "r1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "run": {"info": {
                "run_id": "r1",
                "artifact_uri": "mlflow-artifacts:/1/r1/artifacts"
            }}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let p = client
        .get_run_artifact_path("r1")
        .await
        .expect("get_run_artifact_path should succeed");
    assert_eq!(p, "1/r1/artifacts");
}

/// Test: list_artifacts uses base_path/sub_path in `?path=` and re-prepends
#[tokio::test]
async fn test_list_artifacts_filters_directories() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts"))
        .and(query_param("path", "1/r1/artifacts/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "files": [
                {"path": "log_000001.txt", "is_dir": false, "file_size": 12},
                {"path": "subdir", "is_dir": true},
                {"path": "log_000002.txt", "is_dir": false, "file_size": "34"},
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let entries = client
        .list_artifacts("1/r1/artifacts", Some("logs"))
        .await
        .expect("list_artifacts should succeed");

    assert_eq!(
        entries,
        vec![
            ("logs/log_000001.txt".to_string(), 12),
            ("logs/log_000002.txt".to_string(), 34),
        ],
    );
}

/// Test: list_artifacts at base_path only when sub_path is None
#[tokio::test]
async fn test_list_artifacts_no_sub_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/2.0/mlflow-artifacts/artifacts"))
        .and(query_param("path", "1/r1/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "files": [
                {"path": "logs", "is_dir": true},
                {"path": "stdout.log", "is_dir": false, "file_size": 7},
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let entries = client
        .list_artifacts("1/r1/artifacts", None)
        .await
        .expect("list_artifacts should succeed");
    assert_eq!(entries, vec![("stdout.log".to_string(), 7)]);
}

/// Test: download_artifact prepends base_path to URL path, no run_id query
#[tokio::test]
async fn test_download_artifact_returns_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/2.0/mlflow-artifacts/artifacts/1/r1/artifacts/logs/log_000001.txt",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let bytes = client
        .download_artifact("1/r1/artifacts", "logs/log_000001.txt")
        .await
        .expect("download_artifact should succeed");
    assert_eq!(bytes, b"hello world");
}

/// Test: download_artifact maps 404 to NotFound
#[tokio::test]
async fn test_download_artifact_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/2.0/mlflow-artifacts/artifacts/1/r1/artifacts/logs/missing.txt",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;

    let client = make_client(&server.uri());
    let err = client
        .download_artifact("1/r1/artifacts", "logs/missing.txt")
        .await
        .expect_err("missing artifact should error");
    assert!(matches!(err, xrun_mlflow::MlflowError::NotFound(_)));
}

/// Test: params and tags are sent with log_batch
#[tokio::test]
async fn test_log_batch_with_params_and_tags() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/2.0/mlflow/runs/log-batch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let params = vec![MlflowParam {
        key: "lr".to_string(),
        value: "0.001".to_string(),
    }];
    let tags = vec![MlflowTag {
        key: "source".to_string(),
        value: "xrun".to_string(),
    }];

    let client = make_client(&server.uri());
    client
        .log_batch("run-abc", &params, &[], &tags)
        .await
        .expect("log_batch with params/tags should succeed");

    server.verify().await;
}
