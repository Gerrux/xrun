use xrun_vast::types::{parse_user_info, InstanceInfo, Offer, OfferQuery};

const SEARCH_FIXTURE: &str = include_str!("data/vastai_search_offers.json");
const SHOW_FIXTURE: &str = include_str!("data/vastai_show_instance.json");
const USER_FIXTURE: &[u8] = include_bytes!("data/vastai_show_user.json");

#[test]
fn show_user_fixture_extracts_balance_and_email() {
    let info = parse_user_info(USER_FIXTURE).expect("user fixture parses");
    assert_eq!(info.email.as_deref(), Some("tester@example.com"));
    assert_eq!(info.account_label().as_deref(), Some("tester@example.com"));
    let bal = info.effective_balance().expect("balance present");
    assert!((bal - 12.34).abs() < 1e-9);
}

#[test]
fn show_user_tolerates_missing_fields() {
    let info = parse_user_info(b"{}").expect("empty object parses");
    assert!(info.effective_balance().is_none());
    assert!(info.account_label().is_none());
}

#[test]
fn search_offers_fixture_deserializes() {
    let offers: Vec<Offer> =
        serde_json::from_str(SEARCH_FIXTURE).expect("fixture should parse as Vec<Offer>");
    assert!(
        !offers.is_empty(),
        "fixture must contain at least one offer"
    );
    let first = &offers[0];
    assert_eq!(first.id, 12345);
    assert_eq!(first.gpu_name, "RTX 4090");
    assert_eq!(first.num_gpus, 1);
    assert!((first.gpu_ram - 24.0).abs() < f64::EPSILON);
}

#[test]
fn show_instance_fixture_deserializes_ssh_fields() {
    let info: InstanceInfo =
        serde_json::from_str(SHOW_FIXTURE).expect("fixture should parse as InstanceInfo");
    assert!(
        info.ssh_host
            .as_deref()
            .map(|s: &str| !s.is_empty())
            .unwrap_or(false),
        "ssh_host must be non-empty"
    );
    assert!(info.ssh_port.is_some(), "ssh_port must be present");
}

#[test]
fn offer_query_renders_correctly() {
    let query = OfferQuery {
        gpu_name: "RTX 4090".to_string(),
        gpu_count: 1,
        gpu_ram_gte: Some(24),
        dph_lte: None,
        region: None,
        inet_up_gte: None,
        inet_down_gte: None,
        cuda_gte: None,
        reliability_gte: None,
        direct_port_count_gte: None,
    };
    assert_eq!(query.render(), "gpu_name=RTX_4090 num_gpus=1 gpu_ram>=24");
}

#[test]
fn offer_query_renders_with_all_fields() {
    let query = OfferQuery {
        gpu_name: "A100 SXM4".to_string(),
        gpu_count: 2,
        gpu_ram_gte: Some(80),
        dph_lte: Some(2.5),
        region: Some("us-east".to_string()),
        inet_up_gte: None,
        inet_down_gte: None,
        cuda_gte: None,
        reliability_gte: None,
        direct_port_count_gte: None,
    };
    let rendered = query.render();
    assert!(rendered.contains("gpu_name=A100_SXM4"));
    assert!(rendered.contains("num_gpus=2"));
    assert!(rendered.contains("gpu_ram>=80"));
    assert!(rendered.contains("dph_total<=2.5000"));
    assert!(rendered.contains("datacenter_region=us-east"));
}
