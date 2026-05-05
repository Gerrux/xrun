#![deny(unsafe_code)]

use crate::manifest::types::Vendor;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct MlflowConfig {
    pub url: Option<String>,
    pub experiment_default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PollerConfig {
    pub interval_active_secs: u64,
    pub interval_idle_secs: u64,
}

impl Default for PollerConfig {
    fn default() -> Self {
        Self {
            interval_active_secs: 5,
            interval_idle_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct DefaultsConfig {
    pub vendor: Option<Vendor>,
    pub exp_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TuiConfig {
    pub theme: String,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SearchConfig {
    /// ISO-3166 alpha-2 country codes (case-insensitive) to exclude from offer
    /// search. Vast.ai returns geolocation strings like `"DE, Frankfurt"` —
    /// matching is done on the leading 2-char prefix on the client side.
    pub exclude_countries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BudgetConfig {
    /// Hard cap on per-instance lifetime. Auto-destroy when exceeded.
    pub max_lifetime_hours: f64,
    /// Hard cap on per-instance accumulated cost. Auto-destroy when exceeded.
    pub max_cost_per_instance_usd: f64,
    /// Idle window before auto-destroy (no GPU activity). 0 disables idle cap.
    pub idle_timeout_min: f64,
    /// Soft alert threshold for total daily spend (None = no alert).
    pub daily_budget_usd: Option<f64>,
    /// If true, auto-destroy all active instances on daily budget breach.
    pub daily_budget_hard: bool,
    /// Soft alert threshold for monthly spend.
    pub monthly_budget_usd: Option<f64>,
    /// Hourly rate above which a y/N confirm is required.
    pub require_confirm_above_hourly: f64,
    /// Hourly rate above which a typed-string confirm is required.
    pub require_typed_confirm_above_hourly: f64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_lifetime_hours: 8.0,
            max_cost_per_instance_usd: 10.0,
            idle_timeout_min: 30.0,
            daily_budget_usd: None,
            daily_budget_hard: false,
            monthly_budget_usd: None,
            require_confirm_above_hourly: 0.5,
            require_typed_confirm_above_hourly: 2.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct UiConfig {
    /// Set to `true` once the first-run wizard has been dismissed (either by
    /// completing it or skipping). When `false`, `xrun` (TTY) and `xrun init`
    /// auto-launch the wizard before any other screen.
    pub wizard_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsConfig {
    /// Mirror sinks to fan-out metrics to in addition to the local SQLite +
    /// JSONL store. Empty = local-only (TUI still updates live via the poller).
    /// Currently recognised values: `"mlflow"`, `"wandb"`. (`"comet"` arrives
    /// in v0.8.)
    pub sinks: Vec<String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            sinks: vec!["mlflow".to_string()],
        }
    }
}

/// Per-vendor adapter defaults. The TOML key under `[vendors.<name>]` matches
/// `Vendor::as_str()`. Fields here are *defaults* — manifests override
/// per-experiment. Adding a field requires no per-vendor branching: each
/// adapter reads the values it understands and ignores the rest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VendorDefaults {
    /// Default GPU class string (e.g. `RTX_4090`, `A100_PCIE_40`). Adapter-
    /// specific syntax — not validated here, only forwarded.
    pub default_gpu: Option<String>,
    /// Default container image (Vast/RunPod-style adapters).
    pub default_image: Option<String>,
    /// Default region / datacenter / cloud zone hint.
    pub default_region: Option<String>,
    /// Default disk size in GB.
    pub default_disk_gb: Option<u32>,
    /// Hard cap on hourly price the adapter will accept when picking offers.
    pub max_per_hour_usd: Option<f64>,
    /// Free-form adapter-specific knobs that don't deserve their own typed
    /// field. Anything in here is passed through verbatim to the adapter.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct GlobalConfig {
    pub mlflow: MlflowConfig,
    pub poller: PollerConfig,
    pub defaults: DefaultsConfig,
    pub tui: TuiConfig,
    pub search: SearchConfig,
    pub budget: BudgetConfig,
    pub ui: UiConfig,
    pub metrics: MetricsConfig,
    /// Per-vendor adapter defaults keyed by `Vendor::as_str()`. Empty entries
    /// behave the same as a missing entry — adapters fall back to their own
    /// hard-coded defaults.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub vendors: HashMap<String, VendorDefaults>,
}
