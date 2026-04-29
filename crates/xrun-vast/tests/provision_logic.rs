use xrun_vast::{
    cli::Offer,
    error::VastError,
    provision::{filter_excluded_countries, rank_and_select},
};

fn make_offer(id: u64, dph: f64, gpu_ram: f64) -> Offer {
    Offer {
        id,
        gpu_name: "RTX 4090".to_string(),
        num_gpus: 1,
        gpu_ram,
        dph_total: dph,
        ssh_host: None,
        ssh_port: None,
        reliability2: None,
        disk_space: None,
        status: None,
        geolocation: None,
    }
}

#[test]
fn rank_selects_cheapest_of_three() {
    let offers = vec![
        make_offer(1, 0.30, 24.0),
        make_offer(2, 0.20, 24.0),
        make_offer(3, 0.25, 24.0),
    ];
    let selected = rank_and_select(offers, None, "").unwrap();
    assert_eq!(
        selected.id, 2,
        "cheapest offer (id=2, $0.20/h) must be selected"
    );
    assert!((selected.dph_total - 0.20).abs() < 1e-9);
}

#[test]
fn rank_prefers_higher_vram_when_price_tied() {
    let offers = vec![
        make_offer(1, 0.50, 24.0),
        make_offer(2, 0.50, 48.0),
        make_offer(3, 0.50, 16.0),
    ];
    let selected = rank_and_select(offers, None, "").unwrap();
    assert_eq!(
        selected.id, 2,
        "highest vram (48 GB, id=2) must be preferred on price tie"
    );
}

#[test]
fn rank_returns_no_offers_error_when_empty() {
    let err = rank_and_select(vec![], None, "{test-query}").unwrap_err();
    assert!(matches!(err, VastError::NoOffersAvailable(_)));
}

#[test]
fn rank_returns_price_cap_too_low() {
    let offers = vec![make_offer(1, 0.45, 24.0), make_offer(2, 0.60, 24.0)];
    let err = rank_and_select(offers, Some(0.10), "").unwrap_err();
    match err {
        VastError::PriceCapTooLow { cheapest, cap } => {
            assert!((cheapest - 0.45).abs() < 1e-9, "cheapest should be 0.45");
            assert!((cap - 0.10).abs() < 1e-9, "cap should be 0.10");
        }
        other => panic!("expected PriceCapTooLow, got {other:?}"),
    }
}

fn make_offer_geo(id: u64, geo: Option<&str>) -> Offer {
    let mut o = make_offer(id, 0.40, 24.0);
    o.geolocation = geo.map(|s| s.to_string());
    o
}

#[test]
fn filter_drops_excluded_iso_alpha2_anywhere_in_geo() {
    let offers = vec![
        make_offer_geo(1, Some("DE, Frankfurt")),       // -> DE
        make_offer_geo(2, Some("US-CA, Santa Clara")),  // -> US (CA)
        make_offer_geo(3, Some("cn, Shanghai")),        // -> CN
        make_offer_geo(4, Some("PL, Warsaw")),          // -> PL
        make_offer_geo(5, Some("Germany, DE")),         // -> DE (country-name first)
        make_offer_geo(6, Some("Germany")),             // no code → kept
        make_offer_geo(7, None),                        // no geo → kept
    ];
    let kept = filter_excluded_countries(offers, &["cn".into(), "  DE  ".into()]);
    let ids: Vec<u64> = kept.iter().map(|o| o.id).collect();
    assert_eq!(ids, vec![2, 4, 6, 7]);
}

#[test]
fn filter_noop_when_exclude_empty() {
    let offers = vec![make_offer_geo(1, Some("DE"))];
    let kept = filter_excluded_countries(offers, &[]);
    assert_eq!(kept.len(), 1);
}

#[test]
fn rank_accepts_offer_at_exact_price_cap() {
    let offers = vec![make_offer(1, 0.45, 24.0)];
    let selected = rank_and_select(offers, Some(0.45), "").unwrap();
    assert_eq!(selected.id, 1);
}
