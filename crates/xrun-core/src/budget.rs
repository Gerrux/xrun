#![deny(unsafe_code)]

//! Pure budget logic: cap evaluation, cost accumulation, spend aggregation.
//!
//! No I/O — caller passes in `Instance`, config and `now`. The poll-daemon
//! glues this to SQLite and the vendor adapter.

use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::config::BudgetConfig;
use crate::error::StoreError;
use crate::store::{Instance, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyReason {
    LifetimeCap,
    CostCap,
    IdleTimeout,
}

impl DestroyReason {
    pub fn as_str(self) -> &'static str {
        match self {
            DestroyReason::LifetimeCap => "lifetime_cap",
            DestroyReason::CostCap => "cost_cap",
            DestroyReason::IdleTimeout => "idle_timeout",
        }
    }
}

/// Cost accrued from `created_at` to `now` at `price_per_hour`. Returns 0 when
/// either is missing — Vast's billing starts at allocation, but a missing
/// `created_at` means we have no anchor and reporting 0 is safer than guessing.
pub fn accumulate_cost(inst: &Instance, now: DateTime<Utc>) -> f64 {
    let (Some(created), Some(rate)) = (inst.created_at, inst.price_per_hour) else {
        return 0.0;
    };
    let elapsed_secs = (now - created).num_seconds().max(0) as f64;
    rate * (elapsed_secs / 3600.0)
}

/// Evaluate hard caps. Returns the first reason that fires (lifetime → cost →
/// idle). Cost cap uses the *projected* cost at `now`, not the stored
/// `accumulated_cost`, so the daemon catches breaches even between cost-update
/// ticks.
pub fn evaluate_caps(inst: &Instance, now: DateTime<Utc>) -> Option<DestroyReason> {
    if let (Some(created), Some(max_secs)) = (inst.created_at, inst.max_lifetime_secs) {
        if (now - created).num_seconds() >= max_secs {
            return Some(DestroyReason::LifetimeCap);
        }
    }

    if let Some(max_cost) = inst.max_cost_usd {
        let projected = accumulate_cost(inst, now).max(inst.accumulated_cost);
        if projected >= max_cost {
            return Some(DestroyReason::CostCap);
        }
    }

    if let Some(idle_secs) = inst.idle_timeout_secs {
        if idle_secs > 0 {
            // Fall back to created_at when we never observed activity — a brand
            // new instance with no activity is still subject to the idle cap.
            let anchor = inst.last_active_at.or(inst.created_at);
            if let Some(anchor) = anchor {
                if (now - anchor).num_seconds() >= idle_secs {
                    return Some(DestroyReason::IdleTimeout);
                }
            }
        }
    }

    None
}

/// Total spend on a UTC date: completed runs (`runs.cost_usd_estimate`) +
/// live accrual on instances created that day. Live accrual is computed at
/// `now` so the dashboard shows "today so far" without waiting for a tick.
pub fn daily_spend(
    store: &Store,
    day: NaiveDate,
    now: DateTime<Utc>,
) -> Result<f64, StoreError> {
    let mut total = store.sum_run_cost_for_date(day)?;
    for inst in store.list_instances()? {
        let Some(created) = inst.created_at else { continue };
        if created.date_naive() != day {
            continue;
        }
        if inst.destroyed_at.is_some() {
            // Already accounted for via runs.cost_usd_estimate when the run
            // closed; skip to avoid double-counting.
            continue;
        }
        total += accumulate_cost(&inst, now);
    }
    Ok(total)
}

/// Total live burn rate $/hr across active instances. Used by the dashboard
/// "Burn" card and by the runway calculation in the status bar.
pub fn active_hourly_burn(store: &Store) -> Result<f64, StoreError> {
    let active = store.list_active_instances()?;
    Ok(active
        .iter()
        .filter_map(|i| i.price_per_hour)
        .sum())
}

/// Helper: today (UTC) — the dashboard always reports in UTC to match
/// `created_at` semantics in the store.
pub fn today_utc(now: DateTime<Utc>) -> NaiveDate {
    NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())
        .expect("y/m/d came from a valid DateTime")
}

/// Translate `BudgetConfig` defaults into `InstanceCaps` for fresh instances.
pub fn caps_from_config(cfg: &BudgetConfig) -> crate::store::InstanceCaps {
    crate::store::InstanceCaps {
        max_lifetime_secs: if cfg.max_lifetime_hours > 0.0 {
            Some((cfg.max_lifetime_hours * 3600.0) as i64)
        } else {
            None
        },
        max_cost_usd: if cfg.max_cost_per_instance_usd > 0.0 {
            Some(cfg.max_cost_per_instance_usd)
        } else {
            None
        },
        idle_timeout_secs: if cfg.idle_timeout_min > 0.0 {
            Some((cfg.idle_timeout_min * 60.0) as i64)
        } else {
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_instance() -> Instance {
        Instance {
            id: "i1".into(),
            vendor: "vast".into(),
            run_id: None,
            gpu_type: None,
            price_per_hour: Some(1.0),
            created_at: Some(Utc.with_ymd_and_hms(2026, 4, 29, 10, 0, 0).unwrap()),
            destroyed_at: None,
            state_json: None,
            max_lifetime_secs: None,
            max_cost_usd: None,
            idle_timeout_secs: None,
            accumulated_cost: 0.0,
            last_active_at: None,
            auto_destroyed_reason: None,
        }
    }

    #[test]
    fn accumulate_cost_simple() {
        let inst = make_instance();
        let now = inst.created_at.unwrap() + chrono::Duration::hours(2);
        assert!((accumulate_cost(&inst, now) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn accumulate_cost_zero_without_created_at() {
        let mut inst = make_instance();
        inst.created_at = None;
        let now = Utc::now();
        assert_eq!(accumulate_cost(&inst, now), 0.0);
    }

    #[test]
    fn accumulate_cost_zero_without_rate() {
        let mut inst = make_instance();
        inst.price_per_hour = None;
        let now = inst.created_at.unwrap() + chrono::Duration::hours(1);
        assert_eq!(accumulate_cost(&inst, now), 0.0);
    }

    #[test]
    fn cap_lifetime_fires() {
        let mut inst = make_instance();
        inst.max_lifetime_secs = Some(3600);
        let now = inst.created_at.unwrap() + chrono::Duration::seconds(3601);
        assert_eq!(evaluate_caps(&inst, now), Some(DestroyReason::LifetimeCap));
    }

    #[test]
    fn cap_cost_fires_via_projection() {
        let mut inst = make_instance();
        inst.max_cost_usd = Some(0.5);
        // 31 minutes at $1/hr ≈ $0.516; no stored accumulated_cost yet.
        let now = inst.created_at.unwrap() + chrono::Duration::minutes(31);
        assert_eq!(evaluate_caps(&inst, now), Some(DestroyReason::CostCap));
    }

    #[test]
    fn cap_cost_fires_via_stored() {
        let mut inst = make_instance();
        inst.max_cost_usd = Some(5.0);
        inst.accumulated_cost = 5.5;
        // Recent creation, but stored cost already exceeds — still fires.
        let now = inst.created_at.unwrap() + chrono::Duration::minutes(1);
        assert_eq!(evaluate_caps(&inst, now), Some(DestroyReason::CostCap));
    }

    #[test]
    fn cap_idle_uses_last_active() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(600);
        inst.last_active_at =
            Some(inst.created_at.unwrap() + chrono::Duration::minutes(5));
        let now = inst.last_active_at.unwrap() + chrono::Duration::minutes(11);
        assert_eq!(evaluate_caps(&inst, now), Some(DestroyReason::IdleTimeout));
    }

    #[test]
    fn cap_idle_falls_back_to_created_at() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(600);
        // No last_active_at — fall back to created_at.
        let now = inst.created_at.unwrap() + chrono::Duration::minutes(11);
        assert_eq!(evaluate_caps(&inst, now), Some(DestroyReason::IdleTimeout));
    }

    #[test]
    fn cap_idle_zero_disabled() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(0);
        let now = inst.created_at.unwrap() + chrono::Duration::hours(99);
        assert_eq!(evaluate_caps(&inst, now), None);
    }

    #[test]
    fn no_caps_no_destroy() {
        let inst = make_instance();
        let now = inst.created_at.unwrap() + chrono::Duration::hours(99);
        assert_eq!(evaluate_caps(&inst, now), None);
    }

    #[test]
    fn lifetime_takes_priority_over_cost() {
        let mut inst = make_instance();
        inst.max_lifetime_secs = Some(60);
        inst.max_cost_usd = Some(0.001);
        let now = inst.created_at.unwrap() + chrono::Duration::seconds(120);
        // Both would fire, but lifetime is checked first.
        assert_eq!(evaluate_caps(&inst, now), Some(DestroyReason::LifetimeCap));
    }

    #[test]
    fn caps_from_config_zero_disables() {
        let cfg = BudgetConfig {
            max_lifetime_hours: 0.0,
            idle_timeout_min: 0.0,
            ..BudgetConfig::default()
        };
        let caps = caps_from_config(&cfg);
        assert!(caps.max_lifetime_secs.is_none());
        assert!(caps.idle_timeout_secs.is_none());
        assert_eq!(caps.max_cost_usd, Some(10.0));
    }
}
