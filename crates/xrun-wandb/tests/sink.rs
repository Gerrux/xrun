//! Integration tests for `WandbSink` against a wiremock-stubbed wandb API.
//!
//! Validates the *wire-level* expectations our client makes of wandb:
//! - GraphQL `viewer` is hit when entity is unset.
//! - `upsertBucket` mutation is sent on `open_run`.
//! - `file_stream` POST carries history lines + offset on metric batches.
//! - `finalize` sends `complete: true` + the right exit code.
//!
//! No real wandb. The live smoke is run manually with the api key already
//! in `~/.config/xrun/credentials.toml` once slice 4 wires the sink end-to-
//! end. These tests gate the contract.

use std::collections::HashMap;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use xrun_core::metric_sink::{MetricPoint, MetricSink, OpenRunCtx};
use xrun_core::store::RunStatus;
use xrun_wandb::{WandbClient, WandbSink};

fn make_sink(uri: &str, default_entity: Option<String>) -> WandbSink {
    let client = WandbClient::new(uri, "wandb_v1_test_key");
    WandbSink::new(client, default_entity).with_web_base(uri)
}

fn empty_ctx<'a>(
    run_id: &'a str,
    experiment: &'a str,
    config: &'a HashMap<String, serde_json::Value>,
    tags: &'a HashMap<String, String>,
) -> OpenRunCtx<'a> {
    OpenRunCtx {
        run_id,
        experiment,
        run_name: Some("smoke-run"),
        vendor: "kaggle",
        instance_id: Some("kaggle-slug"),
        config,
        tags,
    }
}

fn upsert_response() -> serde_json::Value {
    json!({
        "data": {
            "upsertBucket": {
                "bucket": {
                    "id": "abc-bucket-id",
                    "name": "xrun-01TEST",
                    "displayName": "smoke-run",
                    "project": {
                        "name": "exp-foo",
                        "entity": { "name": "user-from-mock" }
                    }
                }
            }
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_run_resolves_entity_via_viewer_when_default_missing() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        // First wandb call hits viewer; second is upsertBucket. wiremock
        // doesn't distinguish by body, so the same path stub returns
        // viewer once and upsert thereafter — we set the higher-priority
        // upsert mock first so it consumes the second hit.
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "viewer": { "entity": "user-from-mock" } }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(upsert_response()))
        .mount(&server)
        .await;

    let sink = make_sink(&server.uri(), None);
    let cfg = HashMap::new();
    let tags = HashMap::new();
    let ctx = empty_ctx("01TEST", "exp-foo", &cfg, &tags);
    let handle = sink.open_run(ctx).await.expect("open_run");

    assert_eq!(handle.sink_name, "wandb");
    assert_eq!(handle.remote_run_id, "abc-bucket-id");
    assert!(handle
        .remote_url
        .as_deref()
        .unwrap()
        .ends_with("/user-from-mock/exp-foo/runs/xrun-01TEST"));

    // Two graphql posts: viewer + upsert.
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 2, "expected 2 graphql calls (viewer + upsert)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_run_skips_viewer_when_entity_pinned() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(upsert_response()))
        .mount(&server)
        .await;

    let sink = make_sink(&server.uri(), Some("pinned-entity".into()));
    let cfg = HashMap::new();
    let tags = HashMap::new();
    let ctx = empty_ctx("01TEST", "exp-foo", &cfg, &tags);
    sink.open_run(ctx).await.expect("open_run");

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "expected 1 graphql call (upsert only)");

    // Verify upsertBucket variables carry the pinned entity, not user-from-mock.
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let entity = body
        .pointer("/variables/entity")
        .and_then(|v| v.as_str())
        .unwrap();
    assert_eq!(entity, "pinned-entity");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn log_metrics_batch_groups_by_step_and_posts_file_stream() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(upsert_response()))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path(
            "/files/user-from-mock/exp-foo/xrun-01TEST/file_stream",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let sink = make_sink(&server.uri(), Some("user-from-mock".into()));
    let cfg = HashMap::new();
    let tags = HashMap::new();
    let ctx = empty_ctx("01TEST", "exp-foo", &cfg, &tags);
    let handle = sink.open_run(ctx).await.expect("open_run");

    let now = chrono::Utc::now().timestamp_millis();
    let batch = vec![
        MetricPoint {
            key: "loss".into(),
            value: 0.5,
            step: 1,
            timestamp_ms: now,
        },
        MetricPoint {
            key: "acc".into(),
            value: 0.9,
            step: 1, // same step → should land on same HistoryLine
            timestamp_ms: now,
        },
        MetricPoint {
            key: "loss".into(),
            value: 0.4,
            step: 2,
            timestamp_ms: now + 100,
        },
    ];
    sink.log_metrics_batch(&handle, &batch)
        .await
        .expect("log_metrics_batch");

    // file_stream: 1 POST. Body should carry 2 history lines (step=1, step=2).
    let reqs = server.received_requests().await.unwrap();
    let stream_reqs: Vec<_> = reqs
        .iter()
        .filter(|r| r.url.path().contains("file_stream"))
        .collect();
    assert_eq!(stream_reqs.len(), 1, "expected 1 file_stream POST");

    let body: serde_json::Value = serde_json::from_slice(&stream_reqs[0].body).unwrap();
    let content = body
        .pointer("/files/wandb-history.jsonl/content")
        .and_then(|v| v.as_array())
        .expect("content array");
    assert_eq!(content.len(), 2, "expected 2 history lines");

    let line_step1: serde_json::Value = serde_json::from_str(content[0].as_str().unwrap()).unwrap();
    assert_eq!(line_step1["_step"], 1);
    assert!(line_step1.get("loss").is_some(), "loss in step 1");
    assert!(line_step1.get("acc").is_some(), "acc in step 1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalize_sends_complete_with_exit_code() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(upsert_response()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/files/user-from-mock/exp-foo/xrun-01TEST/file_stream",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let sink = make_sink(&server.uri(), Some("user-from-mock".into()));
    let cfg = HashMap::new();
    let tags = HashMap::new();
    let ctx = empty_ctx("01TEST", "exp-foo", &cfg, &tags);
    let handle = sink.open_run(ctx).await.expect("open_run");
    sink.finalize(&handle, RunStatus::Failed)
        .await
        .expect("finalize");

    let reqs = server.received_requests().await.unwrap();
    let stream_reqs: Vec<_> = reqs
        .iter()
        .filter(|r| r.url.path().contains("file_stream"))
        .collect();
    assert_eq!(stream_reqs.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&stream_reqs[0].body).unwrap();
    assert_eq!(body["complete"], true);
    assert_eq!(body["exitcode"], 1, "Failed → exitcode 1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_401_surfaces_as_auth_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let sink = make_sink(&server.uri(), Some("user".into()));
    let cfg = HashMap::new();
    let tags = HashMap::new();
    let ctx = empty_ctx("01TEST", "exp-foo", &cfg, &tags);
    let err = sink.open_run(ctx).await.unwrap_err();
    assert!(
        matches!(err, xrun_core::metric_sink::MetricSinkError::Auth(_)),
        "expected Auth, got {err:?}"
    );
}
