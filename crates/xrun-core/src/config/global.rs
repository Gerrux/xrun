#![deny(unsafe_code)]

use crate::manifest::types::Vendor;
use serde::{Deserialize, Serialize};

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
pub struct GlobalConfig {
    pub mlflow: MlflowConfig,
    pub poller: PollerConfig,
    pub defaults: DefaultsConfig,
    pub tui: TuiConfig,
}
