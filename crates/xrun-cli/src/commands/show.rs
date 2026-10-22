#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use xrun_core::{
    manifest::Manifest,
    paths,
    store::Run,
    vendor::{InstanceHandle, RawRunStatus},
};
use xrun_kaggle::snapshot;

use crate::cli::ShowArgs;
use crate::commands::common::{build_adapter, open_store, resolve_run, vendor_or_vast, AdapterCtx};

/// Result of `--vendor-status`: one vendor query, never retried.
enum VendorStatusOutcome {
    Ok(RawRunStatus),
    Error {
        message: String,
        queried_at: DateTime<Utc>,
    },
}

/// Kaggle answers a throttled account with HTTP 429; the kaggle CLI puts it
/// in stderr as `429 Client Error: Too Many Requests`.
fn is_rate_limited(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("429") || lower.contains("too many requests")
}

fn vendor_error_message(vendor: &str, err: &str) -> String {
    if is_rate_limited(err) {
        format!(
            "{vendor} rate limit (HTTP 429): the vendor is throttling this account; \
             xrun does not retry, try again in a few minutes"
        )
    } else {
        err.trim().to_string()
    }
}

/// Ask the vendor once for its own state of `run`. Nothing is written to the
/// store and the run lifecycle is untouched.
fn query_vendor_status(
    run: &Run,
    handle: Option<&InstanceHandle>,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
) -> VendorStatusOutcome {
    let vendor = vendor_or_vast(&run.vendor);
    let fail = |message: String| VendorStatusOutcome::Error {
        message,
        queried_at: Utc::now(),
    };
    let Some(handle) = handle else {
        return fail("no stored instance handle for this run".to_string());
    };
    let ctx = AdapterCtx::new("show", db_path, runs_dir, config_dir, &run.id);
    let adapter = match build_adapter(vendor, &ctx) {
        Ok(a) => a,
        Err(e) => return fail(format!("{e:#}")),
    };
    match adapter.raw_run_status(handle) {
        Ok(Some(s)) => VendorStatusOutcome::Ok(s),
        Ok(None) => fail(format!("not supported for vendor {vendor}")),
        Err(e) => fail(vendor_error_message(vendor, &e.to_string())),
    }
}

fn vendor_status_json(outcome: &VendorStatusOutcome) -> serde_json::Value {
    match outcome {
        VendorStatusOutcome::Ok(s) => serde_json::json!({
            "raw": s.raw,
            "message": s.message,
            "queried_at": s.queried_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        }),
        VendorStatusOutcome::Error {
            message,
            queried_at,
        } => serde_json::json!({
            "error": message,
            "queried_at": queried_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        }),
    }
}

fn vendor_status_lines(outcome: &VendorStatusOutcome) -> Vec<String> {
    match outcome {
        VendorStatusOutcome::Ok(s) => {
            let mut lines = vec![format!(
                "  vendor_status: {} (queried {})",
                s.raw,
                s.queried_at.format("%Y-%m-%dT%H:%M:%SZ")
            )];
            if let Some(m) = &s.message {
                lines.push(format!("  vendor_message: {m}"));
            }
            lines
        }
        VendorStatusOutcome::Error {
            message,
            queried_at,
        } => vec![format!(
            "  vendor_status: unavailable (queried {}): {message}",
            queried_at.format("%Y-%m-%dT%H:%M:%SZ")
        )],
    }
}

struct DatasetSummary {
    slug: String,
    files: usize,
    total_bytes: u64,
    captured_at: String,
}

fn dataset_summary_for(run_dir: &Path) -> Option<DatasetSummary> {
    let manifest_path = run_dir.join("manifest.yaml");
    let yaml = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest = Manifest::from_yaml_str(&yaml).ok()?;
    let kaggle = manifest.kaggle.as_ref()?;
    let slug = kaggle.dataset.as_deref()?;
    let snapshots_dir = paths::data_dir().ok()?.join("dataset_snapshots");
    let snap = snapshot::load(&snapshots_dir, slug)?;
    let total_bytes: u64 = snap.files.values().map(|e| e.size).sum();
    Some(DatasetSummary {
        slug: snap.slug,
        files: snap.files.len(),
        total_bytes,
        captured_at: snap.captured_at,
    })
}

pub fn run(args: &ShowArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    let store = open_store(db_path)?;
    let run = resolve_run(&store, &args.id)?;
    let id = run.id.clone();

    let events = store.list_events(&id).context("failed to list events")?;
    let metric_keys = store
        .list_metric_summaries(&id)
        .context("failed to list metric keys")?;

    // Pull ssh_host/port from the instance handle persisted at provision time.
    // Active runs hit this constantly during debugging — without it every
    // `vastai ssh` command starts with a `xrun show | grep ssh` round-trip.
    let handle = run
        .instance_id
        .as_deref()
        .and_then(|iid| store.get_instance(iid).ok().flatten())
        .and_then(|inst| inst.state_json)
        .and_then(|s| serde_json::from_str::<InstanceHandle>(&s).ok());
    let ssh = handle
        .as_ref()
        .and_then(|h| match (h.ssh_host.clone(), h.ssh_port) {
            (Some(host), Some(port)) if !host.is_empty() => Some((host, port, h.ssh_user.clone())),
            _ => None,
        });

    let vendor_status = args
        .vendor_status
        .then(|| query_vendor_status(&run, handle.as_ref(), db_path, runs_dir, config_dir));

    let run_dir = runs_dir.join(run.id.to_string());
    let dataset = dataset_summary_for(&run_dir);

    if args.json {
        let ssh_json = ssh.as_ref().map(|(h, p, u)| {
            serde_json::json!({
                "host": h, "port": p, "user": u, "command": format!("ssh -p {p} {u}@{h}")
            })
        });
        let dataset_json = dataset.as_ref().map(|d| {
            serde_json::json!({
                "slug": d.slug,
                "files": d.files,
                "total_bytes": d.total_bytes,
                "last_pushed_at": d.captured_at,
            })
        });
        let mut out = serde_json::json!({
            "run": run,
            "events": events,
            "metric_keys": metric_keys.iter().map(|m| serde_json::json!({
                "key": m.key,
                "count": m.count,
                "last_step": m.last_step,
                "last": m.last_value,
            })).collect::<Vec<_>>(),
            "ssh": ssh_json,
            "dataset": dataset_json,
        });
        if let Some(vs) = &vendor_status {
            out["vendor_status"] = vendor_status_json(vs);
        }
        println!("{out}");
    } else {
        println!("Run: {}", run.id);
        println!("  name:          {}", run.name);
        println!("  status:        {}", run.status.as_str());
        if let Some(vs) = &vendor_status {
            for line in vendor_status_lines(vs) {
                println!("{line}");
            }
        }
        println!("  vendor:        {}", run.vendor);
        println!(
            "  manifest_hash: {}",
            run.manifest_hash.get(..16).unwrap_or(&run.manifest_hash)
        );
        println!("  manifest_path: {}", run.manifest_path);
        println!(
            "  created_at:    {}",
            run.created_at.format("%Y-%m-%dT%H:%M:%SZ")
        );
        println!(
            "  started_at:    {}",
            run.started_at
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "  ended_at:      {}",
            run.ended_at
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            "  cost_usd:      {}",
            run.cost_usd
                .map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "-".to_string())
        );
        if let Some((host, port, user)) = &ssh {
            println!("  ssh:           {user}@{host}:{port}");
            println!("  ssh_command:   ssh -p {port} {user}@{host}");
        }
        if let Some(d) = &dataset {
            let mb = d.total_bytes as f64 / (1024.0 * 1024.0);
            println!(
                "  dataset:       {} ({} files, {:.1} MiB; last push {})",
                d.slug, d.files, mb, d.captured_at
            );
        }
        println!();
        println!("  run_dir: {}", run_dir.display());
        println!();
        println!("Events ({}):", events.len());
        if events.is_empty() {
            println!("  (none)");
        } else {
            println!("  {:<24}  {:<20}  {:<8}  msg", "ts", "stage", "status");
            for e in &events {
                println!(
                    "  {:<24}  {:<20}  {:<8}  {}",
                    e.ts.format("%Y-%m-%dT%H:%M:%SZ"),
                    e.stage,
                    e.status,
                    e.msg.as_deref().unwrap_or("")
                );
            }
        }
        println!();
        println!("Metric keys ({}):", metric_keys.len());
        if metric_keys.is_empty() {
            println!("  (none)");
        } else {
            for m in &metric_keys {
                println!(
                    "  {}: {} points, last={} (step {})",
                    m.key,
                    m.count,
                    fmt_metric_value(m.last_value),
                    m.last_step
                );
            }
        }
    }

    Ok(())
}

/// Four decimals for scores and losses; scientific notation for small
/// magnitudes (learning rates), which `{:.4}` would print as `0.0000`.
fn fmt_metric_value(v: f64) -> String {
    if v != 0.0 && v.abs() < 1e-3 {
        format!("{v:.3e}")
    } else {
        format!("{v:.4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn metric_value_keeps_small_magnitudes_readable() {
        assert_eq!(fmt_metric_value(0.6312), "0.6312");
        assert_eq!(fmt_metric_value(0.0), "0.0000");
        assert_eq!(fmt_metric_value(5e-4), "5.000e-4");
        assert_eq!(fmt_metric_value(-2.5), "-2.5000");
    }

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 9, 30, 0).unwrap()
    }

    #[test]
    fn queued_kernel_shows_raw_token_and_query_time() {
        let o = VendorStatusOutcome::Ok(RawRunStatus {
            raw: "KernelWorkerStatus.QUEUED".into(),
            message: None,
            queried_at: at(),
        });
        assert_eq!(
            vendor_status_lines(&o),
            vec!["  vendor_status: KernelWorkerStatus.QUEUED (queried 2026-10-14T09:30:00Z)"]
        );
        let j = vendor_status_json(&o);
        assert_eq!(j["raw"], "KernelWorkerStatus.QUEUED");
        assert!(j["message"].is_null());
        assert_eq!(j["queried_at"], "2026-10-14T09:30:00Z");
    }

    #[test]
    fn failure_message_gets_its_own_line_and_json_field() {
        let o = VendorStatusOutcome::Ok(RawRunStatus {
            raw: "KernelWorkerStatus.ERROR".into(),
            message: Some("CUDA out of memory".into()),
            queried_at: at(),
        });
        let lines = vendor_status_lines(&o);
        assert_eq!(lines[1], "  vendor_message: CUDA out of memory");
        assert_eq!(vendor_status_json(&o)["message"], "CUDA out of memory");
    }

    #[test]
    fn rate_limit_is_reported_plainly() {
        let msg = vendor_error_message(
            "kaggle",
            "kaggle CLI failed (exit 1): 429 Client Error: Too Many Requests for url",
        );
        assert!(msg.starts_with("kaggle rate limit (HTTP 429)"), "{msg}");
        assert!(msg.contains("does not retry"));
        let o = VendorStatusOutcome::Error {
            message: msg.clone(),
            queried_at: at(),
        };
        assert_eq!(vendor_status_json(&o)["error"], msg.as_str());
        assert!(vendor_status_lines(&o)[0].starts_with("  vendor_status: unavailable"));
    }

    #[test]
    fn other_errors_pass_through() {
        assert_eq!(
            vendor_error_message("kaggle", " kaggle binary not found in PATH: x \n"),
            "kaggle binary not found in PATH: x"
        );
    }
}
