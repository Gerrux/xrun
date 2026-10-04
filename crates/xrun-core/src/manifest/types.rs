#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Vast,
    Kaggle,
    Local,
    Ssh,
}

impl Vendor {
    /// Canonical lowercase name used in TOML/JSON, on the wire, and as DB key.
    /// Mirrors `#[serde(rename_all = "lowercase")]` so the two stay in sync.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Vendor::Vast => "vast",
            Vendor::Kaggle => "kaggle",
            Vendor::Local => "local",
            Vendor::Ssh => "ssh",
        }
    }

    /// All variants. Single source of truth — extend by adding the variant
    /// here when wiring up a new adapter.
    pub const fn all() -> &'static [Vendor] {
        &[Vendor::Vast, Vendor::Kaggle, Vendor::Local, Vendor::Ssh]
    }
}

impl fmt::Display for Vendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Vendor {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "vast" => Ok(Vendor::Vast),
            "kaggle" => Ok(Vendor::Kaggle),
            "local" => Ok(Vendor::Local),
            "ssh" => Ok(Vendor::Ssh),
            other => Err(format!(
                "unknown vendor `{other}` (expected one of: {})",
                Vendor::all()
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LocalSpec {
    /// GPU selector hint. `auto` (or unset) → pick the first nvidia-smi GPU.
    /// `cpu` → set `CUDA_VISIBLE_DEVICES=""`. Anything else (e.g. `0`, `0,1`)
    /// is forwarded as `CUDA_VISIBLE_DEVICES`.
    pub gpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SshSpec {
    /// Looks up `[vendors.ssh.<host_alias>]` in credentials.toml for the
    /// host/user/port/key fields. Manifests don't embed connection info
    /// directly so they stay portable across machines.
    pub host_alias: String,
    /// Remote workdir root. Defaults to `/tmp/xrun` on the remote.
    /// Per-run subdir `<workdir>/<run-id>/` is created automatically.
    pub workdir: Option<String>,
    /// Same `CUDA_VISIBLE_DEVICES` semantics as `LocalSpec.gpu`. `None` =
    /// inherit (typically what the remote already exports).
    pub gpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuSpec {
    #[serde(rename = "type")]
    pub gpu_type: String,
    pub count: u32,
    pub vram_min_gb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriceSpec {
    pub max_per_hour: f64,
    pub bid: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VastSpec {
    pub image: String,
    pub gpu: GpuSpec,
    pub disk_gb: Option<u32>,
    pub price: Option<PriceSpec>,
    pub region: Option<String>,
    pub ssh: Option<bool>,
    pub ports: Option<Vec<u16>>,
    /// Minimum upload bandwidth (Mbps). Filters out slow upload hosts.
    pub inet_up_min_mbps: Option<f64>,
    /// Minimum download bandwidth (Mbps).
    pub inet_down_min_mbps: Option<f64>,
    /// Minimum CUDA version (e.g. 12.1). Filters hosts running older drivers.
    pub cuda_min: Option<f64>,
    /// Minimum reliability score (0.0–1.0). vast reports this as a fraction.
    pub reliability_min: Option<f64>,
    /// Minimum number of direct (non-proxied) TCP ports available.
    pub direct_port_count_min: Option<u32>,
    /// Geolocation region filter list (e.g. `[Europe, "North America"]`).
    /// When set, replaces the single `region` field for multi-region matching.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KaggleSpec {
    pub kernel_slug: String,
    pub competition: Option<String>,
    /// Single dataset slug (legacy). Use `datasets` for multiple.
    pub dataset: Option<String>,
    /// Multiple dataset slugs (owner/name). Available at /kaggle/input/<name>/.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub datasets: Vec<String>,
    pub enable_gpu: Option<bool>,
    pub enable_internet: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DataMode {
    Copy,
    Rsync,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DataCompress {
    #[default]
    None,
    Gzip,
    Zstd,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnpackSpec {
    pub format: String,
    pub into: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DataSource {
    pub src: String,
    pub dst: String,
    pub mode: Option<DataMode>,
    pub unpack: Option<UnpackSpec>,
    /// Tar-style exclude patterns applied during upload (e.g. `*.pyc`,
    /// `**/__pycache__`, `data/raw`). Forwarded to `tar --exclude=...`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Compress the tar stream before sending. `gzip` works with vanilla tar
    /// on both ends; `zstd` is faster and gives a 4–6× ratio on text but
    /// requires the `zstd` binary on the remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress: Option<DataCompress>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunSpec {
    pub workdir: Option<String>,
    pub setup: Option<String>,
    pub cmd: Option<String>,
    pub notebook: Option<String>,
    pub args: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeepBest {
    pub metric: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointPull {
    pub on: Option<Vec<String>>,
    pub keep_last: Option<u32>,
    pub keep_best: Option<KeepBest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Checkpoints {
    pub watch: Option<String>,
    pub pull: Option<CheckpointPull>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifacts {
    pub patterns: Option<Vec<String>>,
    pub pull_on: Option<String>,
}

/// Allowed values of `policy.on_done`.
pub const ON_DONE_VALUES: &[&str] = &["stop_instance", "keep"];
/// Allowed values of `policy.on_stage_failed`.
pub const ON_STAGE_FAILED_VALUES: &[&str] = &["stop_instance", "keep", "reprovision"];
/// Allowed values of `artifacts.pull_on`.
pub const PULL_ON_VALUES: &[&str] = &["done"];

/// What the poller does when a run finishes naturally (`done`), resolved
/// from `policy.on_done` + `artifacts.{patterns,pull_on}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DonePolicy {
    /// Destroy the instance after a successful finish (default).
    pub stop_instance: bool,
    /// Remote globs to pull into `runs/<id>/artifacts` before destroying.
    /// Empty = no auto-pull.
    pub pull_patterns: Vec<String>,
    /// `policy.on_done` was written in the manifest. Without it a run that
    /// reused an instance (`--reuse-instance`) behaves as `keep`.
    pub explicit_on_done: bool,
    /// Directory relative patterns are anchored at (vast: `run.workdir`).
    pub anchor_dir: Option<String>,
    /// Safety-net pattern pulled when the instance is about to be destroyed
    /// and no `artifacts.patterns` were given: what `xrun pull --ckpt best`
    /// would fetch. `None` where files are already local (local, ssh,
    /// kaggle's whole-output pull).
    pub guard_pattern: Option<String>,
    /// Natural `done` really calls `vendor.destroy`. False for local/ssh,
    /// where destroy kills the (possibly recycled) PID and saves nothing: the
    /// instance row is only marked destroyed.
    pub kill_remote: bool,
}

impl Default for DonePolicy {
    fn default() -> Self {
        Self {
            stop_instance: true,
            pull_patterns: Vec::new(),
            explicit_on_done: false,
            anchor_dir: None,
            guard_pattern: None,
            kill_remote: true,
        }
    }
}

/// Map `xrun pull --ckpt` to a remote glob. Kaggle ignores it (whole output).
pub fn ckpt_to_remote_pattern(ckpt: &str, artifacts: bool) -> String {
    if artifacts {
        return "**/*".to_string();
    }
    match ckpt {
        "all" => "**/*".to_string(),
        "best" => "**/best*".to_string(),
        "latest" => "**/*.pt".to_string(),
        other => other.to_string(),
    }
}

/// vast globs over ssh from `$HOME`, but training ran in `run.workdir`
/// (default `/workspace`): anchor relative patterns there.
pub fn anchor_vast_pattern(workdir: Option<&str>, pattern: &str) -> String {
    if pattern.starts_with('/') {
        return pattern.to_string();
    }
    let workdir = workdir.unwrap_or("/workspace").trim_end_matches('/');
    format!("{workdir}/{pattern}")
}

/// Anchor for an ssh run's relative artifact patterns: the dir the adapter
/// `cd`s into for training (`run.workdir`). Absolute → as is; relative or
/// `~`-prefixed → `~/…`, which the ssh adapter resolves against the remote
/// home (where a relative `cd` lands). `None` when unset: the adapter then
/// anchors relative patterns at the per-run dir, the default training cwd.
pub fn ssh_workdir_anchor(workdir: Option<&str>) -> Option<String> {
    let w = workdir.map(str::trim).filter(|w| !w.is_empty())?;
    let w = if w.starts_with('/') || w == "~" || w.starts_with("~/") {
        w.to_string()
    } else {
        format!("~/{w}")
    };
    let trimmed = w.trim_end_matches('/');
    Some(if trimmed.is_empty() { "/" } else { trimmed }.to_string())
}

impl DonePolicy {
    /// Resolve from a manifest. Values are validated at parse time; an
    /// unknown `on_done` here (manifest built by hand) falls back to the
    /// default (`stop_instance`) so a typo can never leave an instance billing.
    pub fn from_manifest(manifest: &Manifest) -> Self {
        let stop_instance = !matches!(
            manifest.policy.as_ref().and_then(|p| p.on_done.as_deref()),
            Some("keep")
        );
        let mut pull_patterns: Vec<String> = manifest
            .artifacts
            .as_ref()
            .filter(|a| matches!(a.pull_on.as_deref(), None | Some("done")))
            .and_then(|a| a.patterns.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .collect();
        let mut anchor_dir = None;
        let mut guard_pattern = None;
        let mut kill_remote = true;
        match manifest.vendor {
            // The vast adapter globs over ssh from the login dir ($HOME), but
            // the training ran in `cd <run.workdir>` (default `/workspace`,
            // see xrun-vast `build_launch_command`). Anchor relative patterns
            // there, or every `checkpoints/best*.pt` misses.
            Vendor::Vast => {
                let workdir = manifest.run.workdir.as_deref();
                for p in &mut pull_patterns {
                    *p = anchor_vast_pattern(workdir, p);
                }
                anchor_dir = Some(
                    workdir
                        .unwrap_or("/workspace")
                        .trim_end_matches('/')
                        .to_string(),
                );
                guard_pattern = Some(anchor_vast_pattern(
                    workdir,
                    &ckpt_to_remote_pattern("best", false),
                ));
            }
            // Files already live on this host; destroy would only kill a
            // PID that may be recycled by now.
            Vendor::Local => {
                pull_patterns.clear();
                kill_remote = false;
            }
            // The training ran in `cd <run.workdir>` when it is set (else in
            // the per-run dir, where the adapter anchors relative patterns
            // itself): anchor there, or `checkpoints/best*.pt` misses.
            Vendor::Ssh => {
                kill_remote = false;
                if let Some(dir) = ssh_workdir_anchor(manifest.run.workdir.as_deref()) {
                    for p in &mut pull_patterns {
                        *p = anchor_vast_pattern(Some(&dir), p);
                    }
                    anchor_dir = Some(dir);
                }
            }
            // Kaggle's `pull` ignores the pattern and downloads the whole
            // kernel output (and re-ingests its events.jsonl): one call, not
            // one per pattern.
            Vendor::Kaggle if !pull_patterns.is_empty() => {
                pull_patterns = vec!["**/*".to_string()];
            }
            _ => {}
        }
        Self {
            stop_instance,
            pull_patterns,
            explicit_on_done: manifest
                .policy
                .as_ref()
                .is_some_and(|p| p.on_done.is_some()),
            anchor_dir,
            guard_pattern,
            kill_remote,
        }
    }

    /// Anchor a pattern at `anchor_dir` (no-op without one).
    pub fn anchor(&self, pattern: &str) -> String {
        match &self.anchor_dir {
            Some(dir) => anchor_vast_pattern(Some(dir), pattern),
            None => pattern.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MlflowSpec {
    pub experiment: Option<String>,
    pub log_args_as_params: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    /// Stop the run early when a metric stops improving. Saves the money a
    /// finished-but-still-scheduled run would burn. See [`EarlyStop`].
    pub early_stop: Option<EarlyStop>,
    pub on_stage_failed: Option<String>,
    pub on_idle_minutes: Option<u32>,
    pub on_done: Option<String>,
    /// Per-source upload timeout in seconds. When `None`, uploads have no
    /// deadline (default since v0.3.1 — slow nodes are common). When set, each
    /// `data:` source's tar-pipe is wrapped in `tokio::time::timeout` and the
    /// run fails with `upload: timeout` event on expiry. Picked per-source —
    /// a 4 KB script and a 4 GB dataset don't share one budget.
    pub upload_timeout_secs: Option<u64>,
}

/// Metric-based early stopping, evaluated by the poller on every metric
/// batch. When `metric` has not improved by more than `min_delta` for
/// `patience` consecutive new steps, the poller pulls `pull_pattern`
/// (default `**/best*`) into the run's `artifacts/`, destroys the instance
/// and marks the run `done` with an `early_stop` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EarlyStop {
    /// Metric key as logged via `xrun_hook.metric` (e.g. `val_f1`).
    pub metric: String,
    /// Consecutive non-improving evaluations before stopping.
    pub patience: u32,
    /// `max` (default) — higher is better; `min` — lower is better.
    #[serde(default)]
    pub mode: EarlyStopMode,
    /// Improvement smaller than this does not reset patience.
    #[serde(default)]
    pub min_delta: f64,
    /// Pull artifacts before destroying the instance (default true).
    #[serde(default = "default_true")]
    pub pull: bool,
    /// Remote glob to pull (default `**/best*`).
    pub pull_pattern: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum EarlyStopMode {
    #[default]
    Max,
    Min,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub vendor: Vendor,
    pub vast: Option<VastSpec>,
    pub kaggle: Option<KaggleSpec>,
    pub local: Option<LocalSpec>,
    pub ssh: Option<SshSpec>,
    pub data: Option<Vec<DataSource>>,
    pub run: RunSpec,
    pub checkpoints: Option<Checkpoints>,
    pub artifacts: Option<Artifacts>,
    pub mlflow: Option<MlflowSpec>,
    pub policy: Option<Policy>,
    /// Pre-flight resource floor. `xrun doctor --manifest` compares these
    /// against known vendor limits and fails fast when the manifest asks for
    /// more than the target instance can deliver, so users don't burn GPU-time
    /// learning hardware caps the hard way.
    pub requires: Option<Requires>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    /// Minimum RAM the run needs (GB). Compared against the known vendor
    /// instance limit (e.g. Kaggle P100 ≈ 13 GB, T4 x2 ≈ 13 GB).
    pub ram_gb: Option<u32>,
    /// Minimum free working-disk space (GB). Kaggle's writable disk on
    /// `/kaggle/working` is ~73 GB; vast varies by host.
    pub disk_gb: Option<u32>,
}
