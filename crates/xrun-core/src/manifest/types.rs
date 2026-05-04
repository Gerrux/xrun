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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MlflowSpec {
    pub experiment: Option<String>,
    pub log_args_as_params: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
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
