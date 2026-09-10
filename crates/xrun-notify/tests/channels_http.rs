//! HTTP channels against wiremock: payload shape, auth header, error
//! propagation, and the dedupe/journal path through a real SQLite store.

use chrono::{Duration, Utc};
use tempfile::TempDir;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use xrun_core::{config::NotifyConfig, NewNotifyLog, Store};
use xrun_notify::{
    channels::{Channel, NtfyChannel, TelegramChannel, WebhookChannel},
    messages, Kind, Notification, Notifier, SendOutcome, Skipped,
};

fn note() -> Notification {
    Notification::new(Kind::RunFailed, "run.failed:abc", "❌ resnet failed")
        .body("CUDA OOM")
        .run("abc")
        .tag("x")
}

#[tokio::test(flavor = "multi_thread")]
async fn ntfy_posts_json_with_topic_priority_and_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("authorization", "Bearer tk_secret"))
        .and(body_partial_json(serde_json::json!({
            "topic": "xrun-abc",
            "title": "❌ resnet failed",
            "message": "CUDA OOM",
            "priority": 4,
            "tags": ["x"],
        })))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let ch = NtfyChannel::new(
        format!("{}/xrun-abc", server.uri()),
        Some("tk_secret".into()),
    )
    .unwrap();
    tokio::task::spawn_blocking(move || ch.send(&note()).unwrap())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ntfy_surfaces_http_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden topic"))
        .mount(&server)
        .await;
    let ch = NtfyChannel::new(format!("{}/t", server.uri()), None).unwrap();
    let err = tokio::task::spawn_blocking(move || ch.send(&note()).unwrap_err())
        .await
        .unwrap();
    let s = err.to_string();
    assert!(s.contains("403"), "{s}");
    assert!(s.contains("forbidden topic"), "{s}");
}

#[tokio::test(flavor = "multi_thread")]
async fn telegram_sends_plain_text_to_chat() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot123:abc/sendMessage"))
        .and(body_partial_json(serde_json::json!({
            "chat_id": "42",
            "text": "❌ resnet failed\n\nCUDA OOM",
            "disable_notification": false,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    let ch = TelegramChannel::new(
        format!("{}/bot123:abc/sendMessage", server.uri()),
        "42".into(),
    )
    .unwrap();
    tokio::task::spawn_blocking(move || ch.send(&note()).unwrap())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn webhook_posts_structured_payload() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_partial_json(serde_json::json!({
            "source": "xrun",
            "kind": "run.failed",
            "priority": "high",
            "run_id": "abc",
            "text": "❌ resnet failed\n\nCUDA OOM",
            "content": "❌ resnet failed\n\nCUDA OOM",
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let ch = WebhookChannel::new(format!("{}/hook", server.uri())).unwrap();
    tokio::task::spawn_blocking(move || ch.send(&note()).unwrap())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn one_failing_channel_does_not_block_the_other() {
    let bad = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&bad)
        .await;
    let good = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&good)
        .await;

    let channels: Vec<Box<dyn Channel>> = vec![
        Box::new(WebhookChannel::new(format!("{}/x", bad.uri())).unwrap()),
        Box::new(WebhookChannel::new(format!("{}/x", good.uri())).unwrap()),
    ];
    let nt = Notifier::with_channels(NotifyConfig::default(), channels);
    let outcome = tokio::task::spawn_blocking(move || nt.send(None, &note()))
        .await
        .unwrap();
    match outcome {
        SendOutcome::Sent(d) => {
            assert_eq!(d.len(), 2);
            assert!(!d[0].ok);
            assert!(d[1].ok);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn dedupe_uses_journal_and_respects_window() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("runs.db")).unwrap();

    struct Ok_;
    impl Channel for Ok_ {
        fn name(&self) -> &'static str {
            "ok"
        }
        fn send(&self, _: &Notification) -> Result<(), xrun_notify::NotifyError> {
            Ok(())
        }
    }

    let cfg = NotifyConfig {
        dedupe_min: 60,
        ..NotifyConfig::default()
    };
    let nt = Notifier::with_channels(cfg, vec![Box::new(Ok_)]);
    let n = messages::poller_dead(
        &messages::RunRef {
            id: "r1".into(),
            name: "n".into(),
            vendor: "vast".into(),
            instance_id: Some("i".into()),
        },
        None,
        Some(1.0),
        false,
    );

    assert!(matches!(
        nt.send(Some(&mut store), &n),
        SendOutcome::Sent(_)
    ));
    assert!(matches!(
        nt.send(Some(&mut store), &n),
        SendOutcome::Skipped(Skipped::Deduped)
    ));

    // A failed delivery does not count as "sent" for dedupe purposes.
    store
        .append_notify_log(NewNotifyLog {
            ts: Utc::now(),
            run_id: Some("r2"),
            kind: "poller.dead",
            dedupe_key: "poller.dead:r2",
            channel: "ok",
            ok: false,
            title: "t",
            body: None,
            error: Some("boom"),
        })
        .unwrap();
    assert_eq!(store.last_notify_sent("poller.dead:r2").unwrap(), None);

    // An old success is outside the window.
    store
        .append_notify_log(NewNotifyLog {
            ts: Utc::now() - Duration::hours(2),
            run_id: Some("r3"),
            kind: "poller.dead",
            dedupe_key: "poller.dead:r3",
            channel: "ok",
            ok: true,
            title: "t",
            body: None,
            error: None,
        })
        .unwrap();
    let n3 = Notification::new(Kind::PollerDead, "poller.dead:r3", "t").run("r3");
    assert!(matches!(
        nt.send(Some(&mut store), &n3),
        SendOutcome::Sent(_)
    ));

    // Journal lists newest first and filters by run.
    let all = store.list_notify_log(None, 10).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].run_id.as_deref(), Some("r3"));
    let r1 = store.list_notify_log(Some("r1"), 10).unwrap();
    assert_eq!(r1.len(), 1);
    assert_eq!(r1[0].kind, "poller.dead");
}

#[tokio::test(flavor = "multi_thread")]
async fn telegram_poll_updates_parses_messages_and_offset() {
    use wiremock::matchers::query_param;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/bot123:abc/getUpdates"))
        .and(query_param("offset", "42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [
                {"update_id": 42, "message": {"chat": {"id": 7}, "text": "/status"}},
                {"update_id": 43, "message": {"chat": {"id": "8"}, "text": " /stop abc "}},
                {"update_id": 44, "edited_message": {"chat": {"id": 7}, "text": "x"}},
                {"update_id": 45, "message": {"chat": {"id": 7}}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let ch = TelegramChannel::new(
        format!("{}/bot123:abc/sendMessage", server.uri()),
        "7".into(),
    )
    .unwrap();
    let ups = tokio::task::spawn_blocking(move || ch.poll_updates(Some(42)).unwrap())
        .await
        .unwrap();
    assert_eq!(ups.len(), 3);
    assert_eq!(ups[0].update_id, 42);
    assert_eq!(ups[0].chat_id, "7");
    assert_eq!(ups[0].text, "/status");
    assert_eq!(ups[1].chat_id, "8");
    assert_eq!(ups[1].text, "/stop abc");
    assert_eq!(ups[2].text, "");
}

#[tokio::test(flavor = "multi_thread")]
async fn telegram_poll_updates_surfaces_api_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "ok": false, "description": "Unauthorized"
        })))
        .mount(&server)
        .await;
    let ch =
        TelegramChannel::new(format!("{}/botbad/sendMessage", server.uri()), "7".into()).unwrap();
    let err = tokio::task::spawn_blocking(move || ch.poll_updates(None).unwrap_err())
        .await
        .unwrap();
    assert!(err.to_string().contains("Unauthorized"), "{err}");
}
