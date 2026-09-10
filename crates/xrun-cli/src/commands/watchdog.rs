#![deny(unsafe_code)]

//! `xrun watchdog` — the check the poll-daemon cannot do on itself.
//!
//! A dead daemon (reboot, blackout, `cargo install --force` replacing the
//! binary on Windows, OOM on the host) leaves a `running` row with a
//! silent instance that keeps billing. The daemon obviously can't notify
//! about its own death, so this command does, from the outside:
//!
//! 1. For every run in `running`: is the recorded poller PID alive, and is
//!    `poller_heartbeat_at` younger than `[notify].heartbeat_stale_min`?
//!    If not → `poller.dead` notification, and (unless `--no-respawn`) the
//!    same respawn/reconcile path as `xrun resume`.
//! 2. For every non-destroyed *billable* instance in the DB: does a live
//!    run own it? If not → `instance.orphan` notification. Nothing is
//!    destroyed here — that's `xrun gc`'s explicit job.
//!
//! Meant to run every few minutes from cron / Task Scheduler, and it is
//! also what the TUI's 60 s resume tick will grow into. Dedupe via
//! `notify_log` keeps the two from double-pinging.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use clap::{Args, Subcommand};
use serde::Serialize;
use xrun_core::{store::RunStatus, GlobalConfig, Run, Store, VendorAdapter};
use xrun_local::process::process_alive;
use xrun_notify::{
    messages::{self, RunRef},
    Notifier,
};

use crate::commands::notify_cmd::build_notifier;
use crate::commands::resume::{resume_one, Outcome};

#[derive(Args)]
pub struct WatchdogArgs {
    #[command(subcommand)]
    pub command: Option<WatchdogSub>,
    /// Report only: no respawn, no notifications.
    #[arg(long)]
    pub dry_run: bool,
    /// Notify but do not respawn dead pollers.
    #[arg(long)]
    pub no_respawn: bool,
    /// Override `[notify].heartbeat_stale_min`.
    #[arg(long, value_name = "MIN")]
    pub stale_min: Option<u64>,
    /// Skip the vendor-side instance listing (no network call to vast).
    #[arg(long)]
    pub no_vendor: bool,
    /// Skip processing Telegram commands (/status, /stop, /pull).
    #[arg(long)]
    pub no_commands: bool,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum WatchdogSub {
    /// Register / remove / inspect the OS scheduler entry (Task Scheduler on
    /// Windows, crontab elsewhere) that runs `xrun watchdog` periodically.
    Schedule(crate::commands::watchdog_schedule::ScheduleArgs),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollerState {
    /// PID alive and heartbeat fresh (or PID alive with no heartbeat yet).
    Alive,
    /// PID alive but the heartbeat is stale — the loop is stuck.
    Hung,
    /// No live PID and the heartbeat is stale or absent.
    Dead,
}

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub run_id: String,
    pub name: String,
    pub vendor: String,
    pub state: PollerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poller_pid: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_age_secs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Outcome of the resume attempt, when one was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    pub notified: bool,
}

#[derive(Debug, Serialize)]
pub struct OrphanReport {
    pub instance_id: String,
    pub vendor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub age_secs: i64,
    pub cost_usd: f64,
    pub notified: bool,
    /// `db` — known to xrun but nobody polls it; `vendor` — running on the
    /// vendor's side but absent from (or already marked destroyed in) the DB.
    pub source: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub checked_runs: usize,
    pub runs: Vec<RunReport>,
    pub orphans: Vec<OrphanReport>,
    /// Telegram commands handled this pass (human-readable log lines).
    pub commands: Vec<String>,
    pub warnings: Vec<String>,
}

fn classify(run: &Run, stale: Duration, now: chrono::DateTime<Utc>) -> (PollerState, Option<i64>) {
    let pid_alive = run
        .poller_pid
        .filter(|p| *p > 0 && *p <= u32::MAX as i64)
        .map(|p| process_alive(p as u32));
    let hb_age = run.poller_heartbeat_at.map(|t| (now - t).num_seconds());
    let hb_fresh = run.poller_heartbeat_at.map(|t| now - t < stale);
    let state = match (pid_alive, hb_fresh) {
        (Some(true), Some(true)) | (Some(true), None) => PollerState::Alive,
        (Some(true), Some(false)) => PollerState::Hung,
        // No PID on record: foreground `xrun launch` (no --detach) never
        // records one, so the heartbeat is the only signal.
        (_, Some(true)) => PollerState::Alive,
        (_, Some(false)) => PollerState::Dead,
        // Neither: a run that started before the heartbeat column existed,
        // or a poller that died before its first tick. Give it the same
        // grace window measured from start.
        (_, None) => {
            let anchor = run.started_at.unwrap_or(run.created_at);
            if now - anchor < stale {
                PollerState::Alive
            } else {
                PollerState::Dead
            }
        }
    };
    (state, hb_age)
}

pub fn run(args: &WatchdogArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    if let Some(WatchdogSub::Schedule(sched)) = &args.command {
        return crate::commands::watchdog_schedule::run(sched);
    }
    let global = GlobalConfig::load(config_dir).unwrap_or_default();
    let stale_min = args
        .stale_min
        .unwrap_or(global.notify.heartbeat_stale_min)
        .max(1);
    let stale = Duration::minutes(stale_min as i64);
    let now = Utc::now();

    let (notifier, warnings) = build_notifier(config_dir);
    let mut store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let running: Vec<Run> = store
        .list_active_runs()?
        .into_iter()
        .filter(|r| r.status == RunStatus::Running)
        .collect();

    let mut report = Report {
        checked_runs: running.len(),
        runs: Vec::new(),
        orphans: Vec::new(),
        commands: Vec::new(),
        warnings,
    };

    for run in &running {
        let (state, hb_age) = classify(run, stale, now);
        let cost = run
            .instance_id
            .as_deref()
            .and_then(|id| store.get_instance(id).ok().flatten())
            .map(|i| i.accumulated_cost)
            .filter(|c| *c > 0.0)
            .or(run.cost_usd);
        let mut entry = RunReport {
            run_id: run.id.to_string(),
            name: run.name.clone(),
            vendor: run.vendor.clone(),
            state,
            poller_pid: run.poller_pid,
            heartbeat_age_secs: hb_age,
            cost_usd: cost,
            resume: None,
            notified: false,
        };
        if matches!(entry.state, PollerState::Alive) || args.dry_run {
            report.runs.push(entry);
            continue;
        }

        // Dead or hung. Try to bring it back first so the notification can
        // say what happened, then notify.
        let mut respawned = false;
        let mut reconciled_to: Option<String> = None;
        if !args.no_respawn && matches!(entry.state, PollerState::Dead) {
            match resume_one(run, db_path, runs_dir, config_dir, false) {
                Ok(r) => {
                    respawned = matches!(r.outcome, Outcome::Respawned);
                    if matches!(r.outcome, Outcome::Reconciled) {
                        reconciled_to = r.final_status.clone();
                    }
                    entry.resume = Some(
                        serde_json::to_value(&r.outcome)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_else(|| "unknown".into()),
                    );
                }
                Err(e) => entry.resume = Some(format!("error: {e}")),
            }
        }

        let run_ref = RunRef {
            id: run.id.to_string(),
            name: run.name.clone(),
            vendor: run.vendor.clone(),
            instance_id: run.instance_id.clone(),
        };
        let n = match reconciled_to {
            // Instance is already gone: no money at risk, but the run ended
            // unobserved — say so as a failure rather than a dead poller.
            Some(final_status) => messages::run_failed(
                &run_ref,
                &format!(
                    "poller died and the instance was gone when the watchdog looked \
                     (final status {final_status})"
                ),
                cost,
            ),
            None => messages::poller_dead(&run_ref, run.poller_heartbeat_at, cost, respawned),
        };
        entry.notified = notifier.send(Some(&mut store), &n).any_ok();
        report.runs.push(entry);
    }

    // Orphan instances: billable, not destroyed, and nobody is polling.
    let live_run_ids: Vec<String> = running.iter().map(|r| r.id.to_string()).collect();
    for inst in store.list_active_instances()? {
        if inst.price_per_hour.unwrap_or(0.0) <= 0.0 {
            continue; // Kaggle / local: nothing to bill
        }
        let owned_by_live = inst
            .run_id
            .as_deref()
            .map(|r| live_run_ids.iter().any(|l| l == r))
            .unwrap_or(false);
        if owned_by_live {
            continue;
        }
        let age = inst
            .created_at
            .map(|c| (now - c).num_seconds())
            .unwrap_or(0);
        let mut entry = OrphanReport {
            instance_id: inst.id.clone(),
            vendor: inst.vendor.clone(),
            run_id: inst.run_id.clone(),
            age_secs: age,
            cost_usd: inst.accumulated_cost,
            notified: false,
            source: "db",
        };
        if !args.dry_run {
            let n = messages::instance_orphan(
                &inst.id,
                &inst.vendor,
                inst.run_id.as_deref(),
                inst.accumulated_cost,
                age,
            );
            entry.notified = notifier.send(Some(&mut store), &n).any_ok();
        }
        report.orphans.push(entry);
    }

    // Vendor-side cross-check: instances the DB never heard of (partial
    // create) or already believes dead. This is the expensive failure mode
    // — nothing local would ever notice it. Vast only; Kaggle is free and
    // local/ssh have no vendor list.
    if !args.no_vendor {
        match vendor_orphans(db_path, config_dir, &store) {
            Ok(list) => {
                for (id, dph, uptime) in list {
                    let cost = dph * (uptime as f64 / 3600.0);
                    let mut entry = OrphanReport {
                        instance_id: id.clone(),
                        vendor: "vast".into(),
                        run_id: None,
                        age_secs: uptime,
                        cost_usd: cost,
                        notified: false,
                        source: "vendor",
                    };
                    if !args.dry_run {
                        let n = messages::instance_orphan(&id, "vast", None, cost, uptime);
                        entry.notified = notifier.send(Some(&mut store), &n).any_ok();
                    }
                    report.orphans.push(entry);
                }
            }
            Err(e) => report.warnings.push(format!("vendor check skipped: {e}")),
        }
    }

    // Inbound control: Telegram commands sent to the bot since last pass.
    if !args.no_commands && !args.dry_run {
        match crate::commands::telegram_ctl::process(config_dir, db_path, runs_dir) {
            Ok(lines) => report.commands = lines,
            Err(e) => report.warnings.push(format!("telegram commands: {e}")),
        }
    }

    print_report(&report, &notifier, args);
    Ok(())
}

/// `(instance_id, $/h, uptime_secs)` for vast instances alive on the vendor
/// that are not active in our DB. Returns `Ok(vec![])` when vast is not
/// configured; `Err` only for a failed API call.
fn vendor_orphans(
    db_path: &Path,
    config_dir: &Path,
    store: &Store,
) -> Result<Vec<(String, f64, i64)>> {
    use xrun_vast::VastAdapter;
    let creds = crate::commands::poll_daemon::resolve_vast_credentials(config_dir);
    if creds.api_key.is_none() {
        return Ok(Vec::new());
    }
    let probe_store = Store::open(db_path)?;
    let probe = VastAdapter::new(creds, probe_store);
    let remote = probe
        .vendor_instances()
        .map_err(|e| anyhow::anyhow!("vast instance list failed: {e}"))?;
    let active: std::collections::HashSet<String> = store
        .list_active_instances()?
        .into_iter()
        .filter(|i| i.vendor == "vast")
        .map(|i| i.id)
        .collect();
    Ok(remote
        .into_iter()
        .filter(|r| !active.contains(&r.id))
        .map(|r| {
            (
                r.id,
                r.dph_total.unwrap_or(0.0),
                r.uptime_secs.unwrap_or(0) as i64,
            )
        })
        .collect())
}

fn print_report(report: &Report, notifier: &Notifier, args: &WatchdogArgs) {
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).unwrap_or_default()
        );
        return;
    }
    for w in &report.warnings {
        println!("! {w}");
    }
    if notifier.is_empty() && !args.dry_run {
        println!("! no notify channels configured — findings are printed only");
    }
    println!(
        "watchdog: {} running run(s), {} orphan instance(s)",
        report.checked_runs,
        report.orphans.len()
    );
    for r in &report.runs {
        let hb = r
            .heartbeat_age_secs
            .map(|s| format!("heartbeat {}s ago", s))
            .unwrap_or_else(|| "no heartbeat".into());
        let pid = r
            .poller_pid
            .map(|p| format!("pid {p}"))
            .unwrap_or_else(|| "no pid".into());
        let state = match r.state {
            PollerState::Alive => "alive",
            PollerState::Hung => "HUNG",
            PollerState::Dead => "DEAD",
        };
        let mut line = format!(
            "  {} {} [{}] {state} ({pid}, {hb})",
            r.run_id, r.name, r.vendor
        );
        if let Some(res) = &r.resume {
            line.push_str(&format!(" → resume: {res}"));
        }
        if r.notified {
            line.push_str(" → notified");
        }
        println!("{line}");
    }
    for o in &report.orphans {
        println!(
            "  ORPHAN({}) {} [{}] run={} age={} ${:.2}{}",
            o.source,
            o.instance_id,
            o.vendor,
            o.run_id.as_deref().unwrap_or("-"),
            messages::fmt_duration(o.age_secs),
            o.cost_usd,
            if o.notified { " → notified" } else { "" }
        );
    }
    for c in &report.commands {
        println!("  telegram: {c}");
    }
    if args.dry_run {
        println!("(dry run — nothing respawned or sent)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrun_core::store::RunId;

    fn run_with(
        pid: Option<i64>,
        hb: Option<chrono::DateTime<Utc>>,
        started: chrono::DateTime<Utc>,
    ) -> Run {
        Run {
            id: RunId::new(),
            name: "t".into(),
            manifest_hash: "h".into(),
            manifest_path: "m".into(),
            vendor: "vast".into(),
            instance_id: Some("i".into()),
            status: RunStatus::Running,
            created_at: started,
            started_at: Some(started),
            ended_at: None,
            cost_usd: None,
            mlflow_run_id: None,
            notes: None,
            poller_pid: pid,
            mlflow_run_url: None,
            wandb_run_id: None,
            wandb_run_url: None,
            poller_heartbeat_at: hb,
        }
    }

    #[test]
    fn fresh_heartbeat_without_pid_is_alive() {
        let now = Utc::now();
        let r = run_with(
            None,
            Some(now - Duration::seconds(30)),
            now - Duration::hours(1),
        );
        let (state, _) = classify(&r, Duration::minutes(5), now);
        assert!(matches!(state, PollerState::Alive));
    }

    #[test]
    fn stale_heartbeat_without_pid_is_dead() {
        let now = Utc::now();
        let r = run_with(
            None,
            Some(now - Duration::minutes(20)),
            now - Duration::hours(1),
        );
        let (state, age) = classify(&r, Duration::minutes(5), now);
        assert!(matches!(state, PollerState::Dead));
        assert!(age.unwrap() >= 1200);
    }

    #[test]
    fn no_signals_uses_grace_from_start() {
        let now = Utc::now();
        let young = run_with(None, None, now - Duration::minutes(1));
        assert!(matches!(
            classify(&young, Duration::minutes(5), now).0,
            PollerState::Alive
        ));
        let old = run_with(None, None, now - Duration::hours(2));
        assert!(matches!(
            classify(&old, Duration::minutes(5), now).0,
            PollerState::Dead
        ));
    }

    #[test]
    fn live_pid_with_stale_heartbeat_is_hung() {
        let now = Utc::now();
        let me = std::process::id() as i64;
        let r = run_with(
            Some(me),
            Some(now - Duration::hours(1)),
            now - Duration::hours(2),
        );
        assert!(matches!(
            classify(&r, Duration::minutes(5), now).0,
            PollerState::Hung
        ));
    }
}
