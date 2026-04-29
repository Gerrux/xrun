#![deny(unsafe_code)]

use std::cmp::Ordering;

use crate::cli::{Offer, OfferQuery};
use crate::error::VastError;
use xrun_core::manifest::VastSpec;

pub fn offer_query_from_manifest(vast: &VastSpec) -> OfferQuery {
    OfferQuery {
        gpu_name: vast.gpu.gpu_type.clone(),
        gpu_count: vast.gpu.count,
        gpu_ram_gte: vast.gpu.vram_min_gb,
        dph_lte: vast.price.as_ref().map(|p| p.max_per_hour),
        region: vast.region.clone(),
    }
}

/// Drop offers whose ISO-3166 alpha-2 country code matches any of the excluded
/// codes (case-insensitive).
///
/// Vast.ai's `geolocation` is a free-form string with at least four observed
/// shapes: `"DE, Frankfurt"`, `"Germany, DE"`, `"US-CA, Santa Clara"`, or just
/// `"Germany"`. A naive 2-char prefix matches the wrong thing on the
/// country-name-first form (e.g. `"Germany, DE"` gets prefix `"GE"` →
/// Georgia). We instead scan for any standalone two-letter all-caps token in
/// the string and treat that as the country code.
///
/// Offers with no extractable code are kept — dropping them would silently
/// shrink the search for no reason.
pub fn filter_excluded_countries(offers: Vec<Offer>, exclude: &[String]) -> Vec<Offer> {
    if exclude.is_empty() {
        return offers;
    }
    let codes: Vec<String> = exclude
        .iter()
        .map(|c| c.trim().to_ascii_uppercase())
        .filter(|c| c.len() == 2 && c.chars().all(|ch| ch.is_ascii_alphabetic()))
        .collect();
    if codes.is_empty() {
        return offers;
    }
    offers
        .into_iter()
        .filter(|o| {
            let Some(geo) = &o.geolocation else { return true };
            let Some(found) = extract_iso_alpha2(geo) else { return true };
            !codes.iter().any(|code| code.as_str() == found.as_str())
        })
        .collect()
}

/// Find the first standalone two-letter all-caps token in `geo`. Boundaries
/// are non-alphabetic ASCII characters (comma, dash, space, slash, …).
fn extract_iso_alpha2(geo: &str) -> Option<String> {
    let upper = geo.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let is_alpha = |b: u8| b.is_ascii_alphabetic();
    let mut i = 0;
    while i < bytes.len() {
        if !is_alpha(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_alpha(bytes[i]) {
            i += 1;
        }
        if i - start == 2 {
            return Some(upper[start..i].to_string());
        }
    }
    None
}

/// Select the best offer from a ranked list.
///
/// Sorts by price ascending, then by VRAM descending for ties.
/// Returns PriceCapTooLow if cheapest offer exceeds the cap.
pub fn rank_and_select(
    mut offers: Vec<Offer>,
    price_cap: Option<f64>,
    query_summary: &str,
) -> Result<Offer, VastError> {
    if offers.is_empty() {
        return Err(VastError::NoOffersAvailable(query_summary.to_string()));
    }

    offers.sort_by(|a, b| {
        match a
            .dph_total
            .partial_cmp(&b.dph_total)
            .unwrap_or(Ordering::Equal)
        {
            Ordering::Equal => b.gpu_ram.partial_cmp(&a.gpu_ram).unwrap_or(Ordering::Equal),
            other => other,
        }
    });

    let cheapest_price = offers[0].dph_total;
    if let Some(cap) = price_cap {
        if cap < cheapest_price {
            return Err(VastError::PriceCapTooLow {
                cheapest: cheapest_price,
                cap,
            });
        }
    }

    Ok(offers.remove(0))
}
