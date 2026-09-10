#![deny(unsafe_code)]

//! Push notifications for xrun: "something happened to your run (or your
//! money) while you weren't looking".
//!
//! The poll-daemon is the only process that knows a run finished, a budget
//! threshold was crossed, or a loss went NaN — so it is also the process
//! that sends the notification. `xrun watchdog` covers the one case the
//! daemon cannot report on: the daemon itself dying while the instance
//! keeps billing.
//!
//! Layout:
//! - [`Notification`] / [`Kind`] — what gets sent.
//! - [`channels`] — how it gets sent (ntfy, Telegram, generic webhook,
//!   desktop toast). Every channel is best-effort and independent: one
//!   failing never blocks the others.
//! - [`Notifier`] — filter (`[notify].events`), dedupe (via `notify_log`),
//!   fan-out, and journaling to SQLite.
//! - [`messages`] — the canonical text for each kind so the daemon, the
//!   watchdog and `xrun notify test` all read the same way.
//! - [`anomaly`] — pure metric-stream checks (NaN / loss spike).

pub mod anomaly;
pub mod channels;
pub mod messages;

use std::time::Duration;

use chrono::Utc;
use thiserror::Error;
use xrun_core::{config::NotifyConfig, Credentials, NewNotifyLog, Store};

pub use channels::Channel;

/// Delivery priority. Maps onto ntfy's 1–5 scale and Telegram's
/// `disable_notification`; desktop toasts use it for urgency hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Default,
    High,
    Urgent,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Default => "default",
            Priority::High => "high",
            Priority::Urgent => "urgent",
        }
    }

    /// ntfy numeric priority (1 = min … 5 = max).
    pub fn ntfy_level(self) -> u8 {
        match self {
            Priority::Low => 2,
            Priority::Default => 3,
            Priority::High => 4,
            Priority::Urgent => 5,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" | "min" => Some(Priority::Low),
            "default" | "normal" => Some(Priority::Default),
            "high" => Some(Priority::High),
            "urgent" | "max" => Some(Priority::Urgent),
            _ => None,
        }
    }
}

/// Notification kinds. The string form is what `[notify].events` matches
/// against and what lands in `notify_log.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    RunDone,
    RunFailed,
    RunIdle,
    RunEarlyStopped,
    BudgetWarn,
    BudgetAutoDestroyed,
    BudgetDaily,
    BudgetMonthly,
    InstanceCleanupFailed,
    InstanceOrphan,
    MetricAnomaly,
    PollerDead,
    /// Free-form message from the training script (`xrun_hook.notify`).
    User,
    Test,
    Manual,
}

impl Kind {
    pub const ALL: &'static [Kind] = &[
        Kind::RunDone,
        Kind::RunFailed,
        Kind::RunIdle,
        Kind::RunEarlyStopped,
        Kind::BudgetWarn,
        Kind::BudgetAutoDestroyed,
        Kind::BudgetDaily,
        Kind::BudgetMonthly,
        Kind::InstanceCleanupFailed,
        Kind::InstanceOrphan,
        Kind::MetricAnomaly,
        Kind::PollerDead,
        Kind::User,
        Kind::Test,
        Kind::Manual,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::RunDone => "run.done",
            Kind::RunFailed => "run.failed",
            Kind::RunIdle => "run.idle",
            Kind::RunEarlyStopped => "run.early_stopped",
            Kind::BudgetWarn => "budget.warn",
            Kind::BudgetAutoDestroyed => "budget.auto_destroyed",
            Kind::BudgetDaily => "budget.daily",
            Kind::BudgetMonthly => "budget.monthly",
            Kind::InstanceCleanupFailed => "instance.cleanup_failed",
            Kind::InstanceOrphan => "instance.orphan",
            Kind::MetricAnomaly => "metric.anomaly",
            Kind::PollerDead => "poller.dead",
            Kind::User => "user",
            Kind::Test => "test",
            Kind::Manual => "manual",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Kind::RunDone => "run finished with done:ok (duration + cost)",
            Kind::RunFailed => "run failed (stage fail, PID gone, vendor error)",
            Kind::RunIdle => "run auto-stopped by the idle timeout",
            Kind::RunEarlyStopped => {
                "policy.early_stop fired: metric plateaued, best checkpoint pulled"
            }
            Kind::BudgetWarn => "instance crossed a [notify].cost_warn_pct threshold of --max-cost",
            Kind::BudgetAutoDestroyed => "instance destroyed by a hard cap (cost / lifetime)",
            Kind::BudgetDaily => "daily spend reached [budget].daily_budget_usd",
            Kind::BudgetMonthly => "month-to-date spend reached [budget].monthly_budget_usd",
            Kind::InstanceCleanupFailed => {
                "vendor refused to destroy the instance — it is still billing"
            }
            Kind::InstanceOrphan => "instance alive in the DB with no run polling it",
            Kind::MetricAnomaly => "NaN/inf in a metric, or a loss spike",
            Kind::PollerDead => "poll-daemon heartbeat stale or PID gone while run is `running`",
            Kind::User => "`xrun_hook.notify(...)` from the training script",
            Kind::Test => "`xrun notify test`",
            Kind::Manual => "`xrun notify send`",
        }
    }

    /// Default priority for the kind. Money-at-risk kinds are urgent so
    /// they punch through ntfy/Telegram quiet modes.
    pub fn default_priority(self) -> Priority {
        match self {
            Kind::RunDone | Kind::RunEarlyStopped | Kind::User | Kind::Test | Kind::Manual => {
                Priority::Default
            }
            Kind::RunFailed | Kind::RunIdle | Kind::BudgetWarn | Kind::MetricAnomaly => {
                Priority::High
            }
            Kind::BudgetAutoDestroyed | Kind::BudgetDaily | Kind::BudgetMonthly => Priority::High,
            Kind::InstanceCleanupFailed | Kind::InstanceOrphan | Kind::PollerDead => {
                Priority::Urgent
            }
        }
    }
}

/// One outgoing notification. Build via [`messages`] for the standard
/// kinds; `Manual` is free-form.
#[derive(Debug, Clone)]
pub struct Notification {
    pub kind: Kind,
    /// Stable identity for dedupe. Same key within `[notify].dedupe_min`
    /// minutes of a successful delivery is dropped.
    pub dedupe_key: String,
    pub run_id: Option<String>,
    pub title: String,
    pub body: String,
    pub priority: Priority,
    /// ntfy tags (emoji shortcodes). Ignored by channels without tags.
    pub tags: Vec<String>,
}

impl Notification {
    pub fn new(kind: Kind, dedupe_key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind,
            dedupe_key: dedupe_key.into(),
            run_id: None,
            title: title.into(),
            body: String::new(),
            priority: kind.default_priority(),
            tags: Vec::new(),
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn run(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub fn priority(mut self, p: Priority) -> Self {
        self.priority = p;
        self
    }

    pub fn tag(mut self, t: impl Into<String>) -> Self {
        self.tags.push(t.into());
        self
    }

    /// `title\n\nbody` — what plain-text channels send.
    pub fn plain_text(&self) -> String {
        if self.body.is_empty() {
            self.title.clone()
        } else {
            format!("{}\n\n{}", self.title, self.body)
        }
    }
}

#[derive(Debug, Error)]
pub enum NotifyError {
    #[error("channel `{channel}` misconfigured: {reason}")]
    Misconfigured {
        channel: &'static str,
        reason: String,
    },
    #[error("http error: {0}")]
    Http(String),
    #[error("{0}")]
    Other(String),
}

/// Outcome of one channel attempt.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Delivery {
    pub channel: &'static str,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Why [`Notifier::send`] delivered nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Skipped {
    /// `[notify].channels` is empty or every listed channel is misconfigured.
    NoChannels,
    /// Kind not selected by `[notify].events`.
    Filtered,
    /// A successful delivery with the same dedupe key is younger than
    /// `[notify].dedupe_min`.
    Deduped,
}

/// Result of a send: either at least one channel was tried, or a reason
/// nothing was.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum SendOutcome {
    Sent(Vec<Delivery>),
    Skipped(Skipped),
}

impl SendOutcome {
    pub fn any_ok(&self) -> bool {
        match self {
            SendOutcome::Sent(d) => d.iter().any(|x| x.ok),
            SendOutcome::Skipped(_) => false,
        }
    }
}

/// Block on a future from sync code. Mirrors `xrun_poller::metric_fanout`:
/// reuse an ambient tokio runtime when present (the Kaggle adapter runs
/// one), else spin up a throwaway current-thread runtime.
pub(crate) fn block<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");
            rt.block_on(fut)
        }
    }
}

/// HTTP timeout for every network channel. Notifications run inside the
/// poll tick, so a hung endpoint must not stall metric ingestion.
pub(crate) const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Kind filter derived from `[notify].events`. `"*"` matches all, exact
/// strings match one kind, `prefix.*` matches a family.
#[derive(Debug, Clone)]
pub struct EventFilter {
    patterns: Vec<String>,
}

impl EventFilter {
    pub fn new(patterns: &[String]) -> Self {
        Self {
            patterns: patterns.iter().map(|p| p.trim().to_string()).collect(),
        }
    }

    pub fn allows(&self, kind: Kind) -> bool {
        // `test`, `manual` and `user` always go through — the user asked
        // for them explicitly (CLI or `xrun_hook.notify`).
        if matches!(kind, Kind::Test | Kind::Manual | Kind::User) {
            return true;
        }
        let k = kind.as_str();
        self.patterns.iter().any(|p| {
            if p == "*" {
                true
            } else if let Some(prefix) = p.strip_suffix(".*") {
                k.starts_with(prefix) && k[prefix.len()..].starts_with('.')
            } else {
                p == k
            }
        })
    }
}

/// Fan-out + filter + dedupe + journal. Cheap to construct; the poller
/// holds one for the lifetime of a run.
pub struct Notifier {
    channels: Vec<Box<dyn Channel>>,
    filter: EventFilter,
    config: NotifyConfig,
}

impl Notifier {
    /// Build from config. Channels named in `[notify].channels` but lacking
    /// credentials are dropped with a warning in `warnings` — the caller
    /// decides whether that's fatal (`xrun notify test`) or not (daemon).
    pub fn from_config(config: &NotifyConfig, creds: &Credentials) -> (Self, Vec<String>) {
        let mut channels: Vec<Box<dyn Channel>> = Vec::new();
        let mut warnings = Vec::new();
        for name in &config.channels {
            match channels::build(name.trim(), creds) {
                Ok(ch) => channels.push(ch),
                Err(e) => warnings.push(e.to_string()),
            }
        }
        (
            Self {
                channels,
                filter: EventFilter::new(&config.events),
                config: config.clone(),
            },
            warnings,
        )
    }

    /// Construct with explicit channels (tests, or custom sinks).
    pub fn with_channels(config: NotifyConfig, channels: Vec<Box<dyn Channel>>) -> Self {
        Self {
            channels,
            filter: EventFilter::new(&config.events),
            config,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    pub fn channel_names(&self) -> Vec<&'static str> {
        self.channels.iter().map(|c| c.name()).collect()
    }

    pub fn config(&self) -> &NotifyConfig {
        &self.config
    }

    /// Cost-warn thresholds, sorted ascending and clamped to 1..=99.
    pub fn cost_warn_pct(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self
            .config
            .cost_warn_pct
            .iter()
            .copied()
            .filter(|p| (1..=99).contains(p))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Filter → dedupe → fan out → journal. `store` is optional so callers
    /// without a DB (dry runs) still get delivery; without it there is no
    /// dedupe and no journal.
    pub fn send(&self, store: Option<&mut Store>, n: &Notification) -> SendOutcome {
        if self.channels.is_empty() {
            return SendOutcome::Skipped(Skipped::NoChannels);
        }
        if !self.filter.allows(n.kind) {
            return SendOutcome::Skipped(Skipped::Filtered);
        }
        if let Some(st) = &store {
            if self.is_duplicate(st, n) {
                return SendOutcome::Skipped(Skipped::Deduped);
            }
        }
        let deliveries = self.fan_out(n);
        if let Some(st) = store {
            self.journal(st, n, &deliveries);
        }
        SendOutcome::Sent(deliveries)
    }

    /// Same as [`send`](Self::send) but bypasses the kind filter and dedupe.
    /// Used by `xrun notify test` / `send`.
    pub fn send_unfiltered(&self, store: Option<&mut Store>, n: &Notification) -> SendOutcome {
        if self.channels.is_empty() {
            return SendOutcome::Skipped(Skipped::NoChannels);
        }
        let deliveries = self.fan_out(n);
        if let Some(st) = store {
            self.journal(st, n, &deliveries);
        }
        SendOutcome::Sent(deliveries)
    }

    fn is_duplicate(&self, store: &Store, n: &Notification) -> bool {
        if self.config.dedupe_min == 0 {
            return false;
        }
        match store.last_notify_sent(&n.dedupe_key) {
            Ok(Some(last)) => {
                let age = Utc::now() - last;
                age < chrono::Duration::minutes(self.config.dedupe_min as i64)
            }
            Ok(None) => false,
            Err(e) => {
                tracing::warn!("notify: dedupe lookup failed ({e}); sending anyway");
                false
            }
        }
    }

    fn fan_out(&self, n: &Notification) -> Vec<Delivery> {
        self.channels
            .iter()
            .map(|ch| match ch.send(n) {
                Ok(()) => {
                    tracing::info!("notify[{}]: {} — {}", ch.name(), n.kind.as_str(), n.title);
                    Delivery {
                        channel: ch.name(),
                        ok: true,
                        error: None,
                    }
                }
                Err(e) => {
                    tracing::warn!("notify[{}] failed: {e}", ch.name());
                    Delivery {
                        channel: ch.name(),
                        ok: false,
                        error: Some(e.to_string()),
                    }
                }
            })
            .collect()
    }

    fn journal(&self, store: &mut Store, n: &Notification, deliveries: &[Delivery]) {
        let now = Utc::now();
        for d in deliveries {
            let body = if n.body.is_empty() {
                None
            } else {
                Some(n.body.as_str())
            };
            let res = store.append_notify_log(NewNotifyLog {
                ts: now,
                run_id: n.run_id.as_deref(),
                kind: n.kind.as_str(),
                dedupe_key: &n.dedupe_key,
                channel: d.channel,
                ok: d.ok,
                title: &n.title,
                body,
                error: d.error.as_deref(),
            });
            if let Err(e) = res {
                tracing::warn!("notify: journal write failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn filter_star_allows_everything() {
        let f = EventFilter::new(&pats(&["*"]));
        for k in Kind::ALL {
            assert!(f.allows(*k), "{}", k.as_str());
        }
    }

    #[test]
    fn filter_exact_and_prefix() {
        let f = EventFilter::new(&pats(&["run.failed", "budget.*"]));
        assert!(f.allows(Kind::RunFailed));
        assert!(!f.allows(Kind::RunDone));
        assert!(f.allows(Kind::BudgetWarn));
        assert!(f.allows(Kind::BudgetDaily));
        assert!(!f.allows(Kind::PollerDead));
        // test/manual always pass
        assert!(f.allows(Kind::Test));
        assert!(f.allows(Kind::Manual));
    }

    #[test]
    fn filter_empty_blocks_all_but_manual() {
        let f = EventFilter::new(&[]);
        assert!(!f.allows(Kind::RunDone));
        assert!(f.allows(Kind::Test));
    }

    #[test]
    fn priority_parse_roundtrip() {
        for p in [
            Priority::Low,
            Priority::Default,
            Priority::High,
            Priority::Urgent,
        ] {
            assert_eq!(Priority::parse(p.as_str()), Some(p));
        }
        assert_eq!(Priority::parse("nope"), None);
    }

    #[test]
    fn cost_warn_pct_is_sorted_and_clamped() {
        let cfg = NotifyConfig {
            cost_warn_pct: vec![80, 50, 0, 100, 80, 120],
            ..NotifyConfig::default()
        };
        let n = Notifier::with_channels(cfg, vec![]);
        assert_eq!(n.cost_warn_pct(), vec![50, 80]);
    }
}
