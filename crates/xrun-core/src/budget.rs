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
///
/// `train_started_at` is the timestamp of the `train_start` event (when
/// available). It's used as the idle-timer anchor when no metric activity has
/// been observed yet, so a freshly-provisioned instance that hasn't begun
/// training isn't subject to the same idle threshold as one whose script has
/// gone silent for 30 minutes. Pass `None` if no train_start event is in the
/// store yet — the function then falls back to `created_at`.
pub fn evaluate_caps(
    inst: &Instance,
    train_started_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DestroyReason> {
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
            if let Some(anchor) = idle_anchor(inst, train_started_at) {
                if (now - anchor).num_seconds() >= idle_secs {
                    return Some(DestroyReason::IdleTimeout);
                }
            }
        }
    }

    None
}

/// Pick the timestamp the idle timer counts from.
///
/// Priority: most recent activity → train_start → instance created_at.
/// `last_active_at` fires once any heartbeat or stdout block lands, so for a
/// running training script it dominates. While the script is still being set
/// up (provision/upload/compile), `train_started_at` keeps the timer from
/// firing on a freshly-created instance that legitimately needs minutes to
/// boot. The final fallback to `created_at` preserves the original behaviour
/// when no events have been written yet (defensive — short timeouts on a
/// brand-new instance are usually a misconfiguration).
pub fn idle_anchor(
    inst: &Instance,
    train_started_at: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    inst.last_active_at.or(train_started_at).or(inst.created_at)
}

/// Total spend on a UTC date: completed runs (`runs.cost_usd_estimate`) +
/// live accrual on instances created that day. Live accrual is computed at
/// `now` so the dashboard shows "today so far" without waiting for a tick.
pub fn daily_spend(store: &Store, day: NaiveDate, now: DateTime<Utc>) -> Result<f64, StoreError> {
    let mut total = store.sum_run_cost_for_date(day)?;
    for inst in store.list_instances()? {
        let Some(created) = inst.created_at else {
            continue;
        };
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
    Ok(active.iter().filter_map(|i| i.price_per_hour).sum())
}

/// Pure-slice variant of `active_hourly_burn`. The TUI keeps the instance list
/// in memory (no Store at render time), so it computes burn from the slice
/// directly. Skips destroyed and auto-destroyed entries.
pub fn active_hourly_burn_slice(instances: &[Instance]) -> f64 {
    instances
        .iter()
        .filter(|i| i.destroyed_at.is_none() && i.auto_destroyed_reason.is_none())
        .filter_map(|i| i.price_per_hour)
        .sum()
}

/// Smallest per-instance "headroom" until a cap fires: `min(max_cost - acc)`.
/// `None` when no active instance has a cost cap. Negative values are clamped
/// to 0 so the dashboard never shows -$0.05.
pub fn cap_left_slice(instances: &[Instance]) -> Option<f64> {
    instances
        .iter()
        .filter(|i| i.destroyed_at.is_none() && i.auto_destroyed_reason.is_none())
        .filter_map(|i| {
            let cap = i.max_cost_usd?;
            Some((cap - i.accumulated_cost).max(0.0))
        })
        .fold(None, |acc, v| Some(acc.map_or(v, |a: f64| a.min(v))))
}

/// Live "today so far" spend computed from in-memory state. Sums:
///   - `accumulated_cost` for active instances created today (live accrual);
///   - `cost_usd` (or estimate) for runs that ENDED today (already-billed).
///
/// Both signals can come from anywhere — the TUI uses whatever it already has
/// loaded, the daemon computes from Store.
pub fn live_spend_today_slice(
    instances: &[Instance],
    completed_runs_today_cost: f64,
    now: DateTime<Utc>,
) -> f64 {
    let today = today_utc(now);
    let live: f64 = instances
        .iter()
        .filter(|i| i.destroyed_at.is_none() && i.auto_destroyed_reason.is_none())
        .filter(|i| {
            i.created_at
                .map(|c| c.date_naive() == today)
                .unwrap_or(false)
        })
        .map(|i| accumulate_cost(i, now))
        .sum();
    completed_runs_today_cost + live
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
        assert_eq!(
            evaluate_caps(&inst, None, now),
            Some(DestroyReason::LifetimeCap)
        );
    }

    #[test]
    fn cap_cost_fires_via_projection() {
        let mut inst = make_instance();
        inst.max_cost_usd = Some(0.5);
        // 31 minutes at $1/hr ≈ $0.516; no stored accumulated_cost yet.
        let now = inst.created_at.unwrap() + chrono::Duration::minutes(31);
        assert_eq!(
            evaluate_caps(&inst, None, now),
            Some(DestroyReason::CostCap)
        );
    }

    #[test]
    fn cap_cost_fires_via_stored() {
        let mut inst = make_instance();
        inst.max_cost_usd = Some(5.0);
        inst.accumulated_cost = 5.5;
        // Recent creation, but stored cost already exceeds — still fires.
        let now = inst.created_at.unwrap() + chrono::Duration::minutes(1);
        assert_eq!(
            evaluate_caps(&inst, None, now),
            Some(DestroyReason::CostCap)
        );
    }

    #[test]
    fn cap_idle_uses_last_active() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(600);
        inst.last_active_at = Some(inst.created_at.unwrap() + chrono::Duration::minutes(5));
        let now = inst.last_active_at.unwrap() + chrono::Duration::minutes(11);
        assert_eq!(
            evaluate_caps(&inst, None, now),
            Some(DestroyReason::IdleTimeout)
        );
    }

    #[test]
    fn cap_idle_falls_back_to_created_at() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(600);
        // No last_active_at, no train_start — fall back to created_at.
        let now = inst.created_at.unwrap() + chrono::Duration::minutes(11);
        assert_eq!(
            evaluate_caps(&inst, None, now),
            Some(DestroyReason::IdleTimeout)
        );
    }

    #[test]
    fn cap_idle_anchors_on_train_start_when_no_last_active() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(600);
        // train_start fired 5 min after provision, no metric activity since.
        // The idle clock should count from train_start, not created_at,
        // so 5 + 9 = 14 min after creation we're still under the 10-min idle
        // threshold relative to train_start.
        let train_start = inst.created_at.unwrap() + chrono::Duration::minutes(5);
        let now = train_start + chrono::Duration::minutes(9);
        assert_eq!(evaluate_caps(&inst, Some(train_start), now), None);
    }

    #[test]
    fn cap_idle_fires_after_train_start_threshold() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(600);
        let train_start = inst.created_at.unwrap() + chrono::Duration::minutes(5);
        let now = train_start + chrono::Duration::minutes(11);
        assert_eq!(
            evaluate_caps(&inst, Some(train_start), now),
            Some(DestroyReason::IdleTimeout)
        );
    }

    #[test]
    fn cap_idle_last_active_dominates_train_start() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(600);
        let train_start = inst.created_at.unwrap() + chrono::Duration::minutes(5);
        // Heartbeat 20 min after train_start — last_active_at wins, idle
        // clock counts from there.
        inst.last_active_at = Some(train_start + chrono::Duration::minutes(20));
        let now = inst.last_active_at.unwrap() + chrono::Duration::minutes(9);
        assert_eq!(evaluate_caps(&inst, Some(train_start), now), None);
    }

    #[test]
    fn idle_anchor_priority_chain() {
        let mut inst = make_instance();
        let train_start = inst.created_at.unwrap() + chrono::Duration::minutes(5);
        let last_active = inst.created_at.unwrap() + chrono::Duration::minutes(20);

        // 1. last_active wins when present.
        inst.last_active_at = Some(last_active);
        assert_eq!(idle_anchor(&inst, Some(train_start)), Some(last_active));

        // 2. train_start fills in when last_active is absent.
        inst.last_active_at = None;
        assert_eq!(idle_anchor(&inst, Some(train_start)), Some(train_start));

        // 3. created_at is the final fallback.
        assert_eq!(idle_anchor(&inst, None), inst.created_at);
    }

    #[test]
    fn cap_idle_zero_disabled() {
        let mut inst = make_instance();
        inst.idle_timeout_secs = Some(0);
        let now = inst.created_at.unwrap() + chrono::Duration::hours(99);
        assert_eq!(evaluate_caps(&inst, None, now), None);
    }

    #[test]
    fn no_caps_no_destroy() {
        let inst = make_instance();
        let now = inst.created_at.unwrap() + chrono::Duration::hours(99);
        assert_eq!(evaluate_caps(&inst, None, now), None);
    }

    #[test]
    fn lifetime_takes_priority_over_cost() {
        let mut inst = make_instance();
        inst.max_lifetime_secs = Some(60);
        inst.max_cost_usd = Some(0.001);
        let now = inst.created_at.unwrap() + chrono::Duration::seconds(120);
        // Both would fire, but lifetime is checked first.
        assert_eq!(
            evaluate_caps(&inst, None, now),
            Some(DestroyReason::LifetimeCap)
        );
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
