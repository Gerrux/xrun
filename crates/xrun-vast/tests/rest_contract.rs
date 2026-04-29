//! Hermetic contract tests for the REST request/response shapes that
//! `xrun-vast` sends to vast.ai. No network. Locks down the payload format so
//! a future refactor can't silently change the wire shape.

use serde_json::Value;
use xrun_vast::cli::{Offer, OfferQuery};
use xrun_vast::rest::build_offer_search_body;

#[test]
fn search_body_includes_default_filters_and_user_query() {
    let q = OfferQuery {
        gpu_name: "RTX 4090".into(),
        gpu_count: 1,
        gpu_ram_gte: Some(24),
        dph_lte: Some(0.5),
        region: Some("US".into()),
    };
    let body = build_offer_search_body(&q, 5.0);

    // Default filters present (verified, external, rentable, rented)
    assert_eq!(body["verified"]["eq"], Value::Bool(true));
    assert_eq!(body["external"]["eq"], Value::Bool(false));
    assert_eq!(body["rentable"]["eq"], Value::Bool(true));
    assert_eq!(body["rented"]["eq"], Value::Bool(false));

    // gpu_name wire form has spaces (legacy CLI's `parse_query` converts
    // user-supplied `RTX_4090` → `RTX 4090` before posting; we mirror that).
    assert_eq!(body["gpu_name"]["eq"], Value::String("RTX 4090".into()));
    assert_eq!(body["num_gpus"]["eq"], Value::Number(1.into()));
    assert_eq!(body["gpu_ram"]["gte"], Value::Number(24.into()));
    assert_eq!(body["dph_total"]["lte"], serde_json::json!(0.5));
    assert_eq!(body["geolocation"]["eq"], Value::String("US".into()));

    // Meta keys
    assert_eq!(body["type"], Value::String("on-demand".into()));
    assert_eq!(body["order"], serde_json::json!([["score", "desc"]]));
    assert_eq!(body["allocated_storage"], serde_json::json!(5.0));
}

#[test]
fn search_body_normalises_underscored_gpu_name_to_spaces() {
    // Regression: when the manifest carries `RTX_4090` (with an underscore,
    // because the user copied a vastai CLI string), the REST body must still
    // send `RTX 4090` — otherwise the API matches zero offers.
    let q = OfferQuery {
        gpu_name: "RTX_4090".into(),
        gpu_count: 1,
        gpu_ram_gte: None,
        dph_lte: None,
        region: None,
    };
    let body = build_offer_search_body(&q, 5.0);
    assert_eq!(body["gpu_name"]["eq"], Value::String("RTX 4090".into()));
}

#[test]
fn search_body_omits_optional_filters_when_unset() {
    let q = OfferQuery {
        gpu_name: "A100".into(),
        gpu_count: 2,
        gpu_ram_gte: None,
        dph_lte: None,
        region: None,
    };
    let body = build_offer_search_body(&q, 5.0);
    assert!(body.get("gpu_ram").is_none());
    assert!(body.get("dph_total").is_none());
    assert!(body.get("geolocation").is_none());
}

#[test]
fn offers_envelope_parses_legacy_fixture_shape() {
    // The /bundles/ response is `{"offers": [<row>, ...]}`. The legacy CLI
    // fixture is just the bare list, so we wrap it here for the contract.
    let bare = include_str!("data/vastai_search_offers.json");
    let wrapped = format!(r#"{{"offers": {bare}}}"#);
    let v: Value = serde_json::from_str(&wrapped).unwrap();
    let offers: Vec<Offer> =
        serde_json::from_value(v["offers"].clone()).expect("offers parse as Vec<Offer>");
    assert!(!offers.is_empty());
    assert_eq!(offers[0].gpu_name, "RTX 4090");
}
