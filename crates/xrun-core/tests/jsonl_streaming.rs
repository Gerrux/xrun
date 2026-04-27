use std::io::BufReader;

use xrun_core::events::{Event, EventStatus, JsonlReader};
use xrun_core::metrics::{Metric, MetricsJsonlReader};

fn event_line(stage: &str, status: &str) -> String {
    format!(
        r#"{{"ts":"2024-01-15T10:30:00Z","stage":"{stage}","status":"{status}"}}"#,
        stage = stage,
        status = status,
    )
}

#[test]
fn parse_valid_event_line() {
    let json = r#"{"ts":"2024-01-15T10:30:00Z","stage":"train_start","status":"start"}"#;
    let reader = BufReader::new(json.as_bytes());
    let mut iter = JsonlReader::new(reader);
    let event: Event = iter.next().unwrap().unwrap();
    assert_eq!(event.stage, "train_start");
    assert_eq!(event.status, EventStatus::Start);
    assert!(event.msg.is_none());
    assert!(event.extra.is_none());
}

#[test]
fn parse_event_with_msg_and_extra() {
    let json = r#"{"ts":"2024-01-15T10:30:00Z","stage":"epoch","status":"progress","msg":"epoch 5/10","extra":{"loss":0.42}}"#;
    let reader = BufReader::new(json.as_bytes());
    let mut iter = JsonlReader::new(reader);
    let event: Event = iter.next().unwrap().unwrap();
    assert_eq!(event.status, EventStatus::Progress);
    assert_eq!(event.msg.as_deref(), Some("epoch 5/10"));
    assert!(event.extra.is_some());
}

#[test]
fn five_lines_one_bad_does_not_stop_iteration() {
    let good1 = event_line("provision", "start");
    let good2 = event_line("upload", "ok");
    let bad = r#"NOT_VALID_JSON{{{"#;
    let good3 = event_line("unpack", "ok");
    let good4 = event_line("train_start", "start");

    let data = format!("{good1}\n{good2}\n{bad}\n{good3}\n{good4}\n");
    let reader = BufReader::new(data.as_bytes());
    let results: Vec<_> = JsonlReader::new(reader).collect();

    assert_eq!(results.len(), 5);
    assert!(results[0].is_ok(), "line 1 should parse");
    assert!(results[1].is_ok(), "line 2 should parse");
    assert!(results[2].is_err(), "line 3 is bad JSON");
    assert!(results[3].is_ok(), "line 4 should parse");
    assert!(results[4].is_ok(), "line 5 should parse");
}

#[test]
fn bytes_consumed_after_three_lines() {
    let line1 = event_line("provision", "start");
    let line2 = event_line("upload", "ok");
    let line3 = event_line("unpack", "ok");
    let line4 = event_line("train_start", "start");

    // Each line ends with \n
    let data = format!("{line1}\n{line2}\n{line3}\n{line4}\n");
    let expected: u64 = (line1.len() + 1 + line2.len() + 1 + line3.len() + 1) as u64;

    let reader = BufReader::new(data.as_bytes());
    let mut iter = JsonlReader::new(reader);
    iter.next().unwrap().unwrap();
    iter.next().unwrap().unwrap();
    iter.next().unwrap().unwrap();

    assert_eq!(iter.bytes_consumed(), expected);
}

#[test]
fn parse_metric_integer_step_f64_value() {
    let json = r#"{"ts":"2024-01-15T10:30:00Z","step":42,"key":"train_loss","value":0.123}"#;
    let reader = BufReader::new(json.as_bytes());
    let mut iter = MetricsJsonlReader::new(reader);
    let metric: Metric = iter.next().unwrap().unwrap();
    assert_eq!(metric.step, 42);
    assert_eq!(metric.key, "train_loss");
    assert!((metric.value - 0.123).abs() < 1e-10);
}

#[test]
fn parse_metric_with_integer_value() {
    let json = r#"{"ts":"2024-01-15T10:30:00Z","step":0,"key":"accuracy","value":1}"#;
    let reader = BufReader::new(json.as_bytes());
    let mut iter = MetricsJsonlReader::new(reader);
    let metric: Metric = iter.next().unwrap().unwrap();
    assert_eq!(metric.step, 0);
    assert!((metric.value - 1.0).abs() < 1e-10);
}

#[test]
fn unknown_event_status_becomes_progress() {
    let json = r#"{"ts":"2024-01-15T10:30:00Z","stage":"custom","status":"weird_unknown"}"#;
    let reader = BufReader::new(json.as_bytes());
    let mut iter = JsonlReader::new(reader);
    let event: Event = iter.next().unwrap().unwrap();
    assert_eq!(event.status, EventStatus::Progress);
}
