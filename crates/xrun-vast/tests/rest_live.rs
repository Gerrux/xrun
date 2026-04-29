//! Live smoke test for the REST provision path. Hits real vast.ai endpoints
//! using a key from the `VAST_API_KEY` env var. Skipped by default because
//! it requires network + a valid key.
//!
//! Run with:
//!   set VAST_API_KEY=...
//!   cargo test -p xrun-vast --test rest_live -- --ignored

use xrun_vast::cli::OfferQuery;
use xrun_vast::rest;

fn key() -> Option<String> {
    std::env::var("VAST_API_KEY").ok().filter(|k| !k.is_empty())
}

#[tokio::test]
#[ignore = "requires VAST_API_KEY env var"]
async fn search_offers_returns_at_least_one_4090() {
    let Some(key) = key() else {
        return;
    };
    let q = OfferQuery {
        gpu_name: "RTX 4090".into(),
        gpu_count: 1,
        gpu_ram_gte: Some(20),
        dph_lte: None,
        region: None,
        inet_up_gte: None,
    };
    let offers = rest::search_offers(&key, &q)
        .await
        .expect("search_offers must succeed with a valid key");
    assert!(
        !offers.is_empty(),
        "expected at least one offer for a generic 4090 query"
    );
    let first = &offers[0];
    assert_eq!(first.num_gpus, 1, "filter num_gpus=1 must be honoured");
    assert!(
        first.gpu_name.replace(' ', "_").contains("4090"),
        "filter gpu_name=RTX_4090 must be honoured, got: {}",
        first.gpu_name
    );
}

#[tokio::test]
#[ignore = "requires VAST_API_KEY env var"]
async fn show_user_via_rest_works() {
    let Some(key) = key() else {
        return;
    };
    let info = rest::show_user(&key)
        .await
        .expect("show_user must succeed with a valid key");
    assert!(
        info.account_label().is_some(),
        "expected an email/username in the response"
    );
}
