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

/// Select the best offer from a ranked list.
///
/// Sorts by price ascending, then by VRAM descending for ties.
/// Returns PriceCapTooLow if cheapest offer exceeds the cap.
pub fn rank_and_select(mut offers: Vec<Offer>, price_cap: Option<f64>) -> Result<Offer, VastError> {
    if offers.is_empty() {
        return Err(VastError::NoOffersAvailable);
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
