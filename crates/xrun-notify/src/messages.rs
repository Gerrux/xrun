#![deny(unsafe_code)]

//! Canonical notification text per kind. Keep these short: ntfy shows the
//! title in the lock-screen banner and the body when expanded; Telegram
//! shows both inline.

use chrono::{DateTime, Utc};

use crate::{Kind, Notification, Priority};

/// The bits of a run every message wants. Built by the caller from a
/// `Store::get_run` row so this crate stays store-agnostic.
#[derive(Debug, Clone)]
pub struct RunRef {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub instance_id: Option<String>,
}

impl RunRef {
    fn short_id(&self) -> String {
        // ULIDs are 26 chars; the tail is what varies between runs.
        let n = self.id.len();
        if n > 8 {
            self.id[n - 8..].to_string()
        } else {
            self.id.clone()
        }
    }

    fn label(&self) -> String {
        format!("{} [{}/{}]", self.name, self.vendor, self.short_id())
    }
}

pub fn fmt_cost(usd: Option<f64>) -> String {
    match usd {
        Some(c) => format!("${c:.2}"),
        None => "n/a".into(),
    }
}

pub fn fmt_duration(secs: i64) -> String {
    let s = secs.max(0);
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{:02}s", s % 60)
    } else {
        format!("{s}s")
    }
}

pub fn run_done(run: &RunRef, duration_secs: Option<i64>, cost_usd: Option<f64>) -> Notification {
    let mut parts = Vec::new();
    if let Some(d) = duration_secs {
        parts.push(format!("took {}", fmt_duration(d)));
    }
    parts.push(format!("cost {}", fmt_cost(cost_usd)));
    Notification::new(
        Kind::RunDone,
        format!("run.done:{}", run.id),
        format!("✅ {} done", run.label()),
    )
    .body(format!("{}\nxrun pull {}", parts.join(", "), run.id))
    .run(&run.id)
    .tag("white_check_mark")
}

pub fn run_failed(run: &RunRef, reason: &str, cost_usd: Option<f64>) -> Notification {
    let reason = truncate(reason, 300);
    Notification::new(
        Kind::RunFailed,
        format!("run.failed:{}", run.id),
        format!("❌ {} failed", run.label()),
    )
    .body(format!(
        "{reason}\ncost so far {}\nxrun logs {}",
        fmt_cost(cost_usd),
        run.id
    ))
    .run(&run.id)
    .tag("x")
}

pub fn run_idle(run: &RunRef, idle_secs: u64, cost_usd: Option<f64>) -> Notification {
    Notification::new(
        Kind::RunIdle,
        format!("run.idle:{}", run.id),
        format!("💤 {} stopped: idle", run.label()),
    )
    .body(format!(
        "no progress for {}; instance destroyed. cost {}",
        fmt_duration(idle_secs as i64),
        fmt_cost(cost_usd)
    ))
    .run(&run.id)
    .tag("zzz")
}

pub fn run_early_stopped(
    run: &RunRef,
    metric: &str,
    best: f64,
    best_step: i64,
    patience: u32,
    pulled: Option<&str>,
    cost_usd: Option<f64>,
) -> Notification {
    let pull_line = match pulled {
        Some(p) => format!("best checkpoint pulled to {p}"),
        None => "artifacts not pulled (policy.early_stop.pull = false)".to_string(),
    };
    Notification::new(
        Kind::RunEarlyStopped,
        format!("run.early_stopped:{}", run.id),
        format!("⏹ {} early-stopped: {metric} plateaued", run.label()),
    )
    .body(format!(
        "best {metric} = {best:.4} at step {best_step}; no improvement for {patience} evals. \
         {pull_line}. cost {}",
        fmt_cost(cost_usd)
    ))
    .run(&run.id)
    .tag("stop_button")
}

pub fn budget_monthly(run: &RunRef, spent: f64, cap: f64) -> Notification {
    let (y, m) = {
        let now = Utc::now();
        (now.format("%Y"), now.format("%m"))
    };
    Notification::new(
        Kind::BudgetMonthly,
        format!("budget.monthly:{y}-{m}"),
        format!("📆 monthly budget hit: ${spent:.2} ≥ ${cap:.2}"),
    )
    .body(format!(
        "month-to-date spend crossed [budget].monthly_budget_usd; triggered while {} was polling. \
         Soft alert only — runs keep going.",
        run.label()
    ))
    .run(&run.id)
    .tag("calendar")
}

/// Message authored by the training script via `xrun_hook.notify(...)`.
pub fn user_note(
    run: &RunRef,
    title: &str,
    body: Option<&str>,
    priority: Priority,
    ts_ms: i64,
) -> Notification {
    Notification::new(
        Kind::User,
        format!("user:{}:{ts_ms}", run.id),
        format!("💬 {} — {}", run.label(), truncate(title, 120)),
    )
    .body(body.map(|b| truncate(b, 500)).unwrap_or_default())
    .run(&run.id)
    .priority(priority)
    .tag("speech_balloon")
}

pub fn budget_warn(
    run: &RunRef,
    instance_id: &str,
    pct: u8,
    accumulated: f64,
    cap: f64,
) -> Notification {
    let priority = if pct >= 80 {
        Priority::High
    } else {
        Priority::Default
    };
    Notification::new(
        Kind::BudgetWarn,
        format!("budget.warn:{instance_id}:{pct}"),
        format!("💸 {} at {pct}% of --max-cost", run.label()),
    )
    .body(format!(
        "${accumulated:.2} of ${cap:.2} spent on {instance_id}. \
         Auto-destroy at 100%. xrun stop {} to end early.",
        run.id
    ))
    .run(&run.id)
    .priority(priority)
    .tag("moneybag")
}

pub fn budget_auto_destroyed(
    run: &RunRef,
    instance_id: &str,
    reason: &str,
    accumulated: f64,
) -> Notification {
    Notification::new(
        Kind::BudgetAutoDestroyed,
        format!("budget.auto_destroyed:{instance_id}"),
        format!("🛑 {} auto-destroyed ({reason})", run.label()),
    )
    .body(format!(
        "budget guard `{reason}` fired on {instance_id}; total ${accumulated:.2}. \
         Raise --max-cost/--max-hours and `xrun rerun {}` to continue.",
        run.id
    ))
    .run(&run.id)
    .tag("octagonal_sign")
}

pub fn budget_daily(run: &RunRef, spent: f64, cap: f64, hard: bool) -> Notification {
    let date = Utc::now().date_naive();
    let action = if hard {
        "hard mode: this run's instance is being destroyed"
    } else {
        "soft alert only; runs keep going"
    };
    Notification::new(
        Kind::BudgetDaily,
        format!("budget.daily:{date}"),
        format!("📅 daily budget hit: ${spent:.2} ≥ ${cap:.2}"),
    )
    .body(format!("triggered by {}. {action}", run.label()))
    .run(&run.id)
    .tag("calendar")
}

pub fn instance_cleanup_failed(run: &RunRef, instance_id: &str, error: &str) -> Notification {
    Notification::new(
        Kind::InstanceCleanupFailed,
        format!("instance.cleanup_failed:{instance_id}"),
        format!("🔥 could not destroy {instance_id} — still billing"),
    )
    .body(format!(
        "{} : {}\nCheck the vendor console now, then `xrun gc`.",
        run.label(),
        truncate(error, 200)
    ))
    .run(&run.id)
    .tag("fire")
}

pub fn instance_orphan(
    instance_id: &str,
    vendor: &str,
    run_id: Option<&str>,
    accumulated: f64,
    age_secs: i64,
) -> Notification {
    let owner = run_id
        .map(|r| format!("run {r} is not polling it"))
        .unwrap_or_else(|| "no run owns it".into());
    let mut n = Notification::new(
        Kind::InstanceOrphan,
        format!("instance.orphan:{instance_id}"),
        format!("🔥 orphan {vendor} instance {instance_id}"),
    )
    .body(format!(
        "alive for {} (${accumulated:.2} so far) and {owner}. \
         `xrun gc` destroys it.",
        fmt_duration(age_secs)
    ))
    .tag("fire");
    if let Some(r) = run_id {
        n = n.run(r);
    }
    n
}

pub fn metric_anomaly(run: &RunRef, key: &str, description: &str) -> Notification {
    Notification::new(
        Kind::MetricAnomaly,
        format!("metric.anomaly:{}:{key}", run.id),
        format!("📉 {} : {key} looks wrong", run.label()),
    )
    .body(format!(
        "{description}\nxrun metrics {} --key {key} --ascii",
        run.id
    ))
    .run(&run.id)
    .tag("chart_with_downwards_trend")
}

pub fn poller_dead(
    run: &RunRef,
    last_heartbeat: Option<DateTime<Utc>>,
    accumulated: Option<f64>,
    respawned: bool,
) -> Notification {
    let since = last_heartbeat
        .map(|t| {
            format!(
                "last heartbeat {} ago",
                fmt_duration((Utc::now() - t).num_seconds())
            )
        })
        .unwrap_or_else(|| "never sent a heartbeat".into());
    let action = if respawned {
        "poller respawned; verify with xrun events".to_string()
    } else {
        format!("run `xrun resume {}`", run.id)
    };
    Notification::new(
        Kind::PollerDead,
        format!("poller.dead:{}", run.id),
        format!("⚠️ poller dead for {}", run.label()),
    )
    .body(format!(
        "{since}; instance {} may still be billing ({} so far). {action}",
        run.instance_id.as_deref().unwrap_or("?"),
        fmt_cost(accumulated)
    ))
    .run(&run.id)
    .tag("warning")
}

pub fn test(channels: &[&str]) -> Notification {
    Notification::new(Kind::Test, "test", "🔔 xrun notifications work")
        .body(format!(
            "channels: {}. Sent {}.",
            channels.join(", "),
            Utc::now().format("%Y-%m-%d %H:%M UTC")
        ))
        .tag("bell")
}

pub fn manual(title: &str, body: &str, run_id: Option<&str>, priority: Priority) -> Notification {
    let mut n = Notification::new(
        Kind::Manual,
        format!("manual:{}", Utc::now().timestamp_millis()),
        title,
    )
    .body(body)
    .priority(priority);
    if let Some(r) = run_id {
        n = n.run(r);
    }
    n
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> RunRef {
        RunRef {
            id: "01HZZZZZZZZZZZZZZZZZZZZZZZ".into(),
            name: "resnet_v2".into(),
            vendor: "vast".into(),
            instance_id: Some("123456".into()),
        }
    }

    #[test]
    fn done_mentions_cost_and_pull() {
        let n = run_done(&run(), Some(3725), Some(1.234));
        assert!(n.title.contains("resnet_v2"));
        assert!(n.body.contains("1h02m"));
        assert!(n.body.contains("$1.23"));
        assert!(n.body.contains("xrun pull"));
        assert_eq!(n.kind, Kind::RunDone);
    }

    #[test]
    fn budget_warn_key_is_per_threshold() {
        let a = budget_warn(&run(), "i1", 50, 5.0, 10.0);
        let b = budget_warn(&run(), "i1", 80, 8.0, 10.0);
        assert_ne!(a.dedupe_key, b.dedupe_key);
        assert_eq!(a.priority, Priority::Default);
        assert_eq!(b.priority, Priority::High);
    }

    #[test]
    fn cleanup_failed_is_urgent() {
        let n = instance_cleanup_failed(&run(), "123456", "boom");
        assert_eq!(n.priority, Priority::Urgent);
        assert!(n.title.contains("still billing"));
    }

    #[test]
    fn durations_render() {
        assert_eq!(fmt_duration(5), "5s");
        assert_eq!(fmt_duration(65), "1m05s");
        assert_eq!(fmt_duration(3600 * 2 + 60 * 7), "2h07m");
    }

    #[test]
    fn truncate_keeps_short_and_marks_long() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdefgh", 5), "abcde…");
    }
}
