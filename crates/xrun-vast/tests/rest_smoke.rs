//! Live smoke test for the REST `show_user` endpoint.
//! Reads the api key from `XRUN_VAST_KEY` env so it never accidentally bakes a
//! real secret into the repo. Ignored by default.

use xrun_vast::rest;

#[tokio::test]
#[ignore = "requires XRUN_VAST_KEY env and network"]
async fn rest_show_user_returns_account_info() {
    let key = std::env::var("XRUN_VAST_KEY").expect("set XRUN_VAST_KEY to run this test");
    let info = rest::show_user(&key)
        .await
        .expect("show_user must succeed with a valid key");
    let label = info.account_label();
    assert!(
        label.is_some(),
        "an account label (email/username) must be present"
    );
    eprintln!(
        "ok — account: {:?}, balance: {:?}",
        label,
        info.effective_balance()
    );
}

#[tokio::test]
#[ignore = "requires XRUN_VAST_KEY env and network"]
async fn rest_show_instances_returns_list() {
    let key = std::env::var("XRUN_VAST_KEY").expect("set XRUN_VAST_KEY to run this test");
    let instances = rest::show_instances(&key)
        .await
        .expect("show_instances must succeed");
    eprintln!("found {} instance(s)", instances.len());
    for inst in &instances {
        eprintln!(
            "  id={} gpu={:?} dph={:?} status={:?} geo={:?}",
            inst.id, inst.gpu_name, inst.dph_total, inst.actual_status, inst.geolocation
        );
    }
}
