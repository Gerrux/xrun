#![deny(unsafe_code)]

//! Pure data types for the vast.ai REST API.
//!
//! These were previously colocated with the (now-removed) `vastai` Python CLI
//! wrappers. They live here independent of any transport so `rest.rs`, the
//! adapter, and the public API can share them.

use serde::Deserialize;

use crate::error::VastError;

pub type InstanceId = u64;

/// Compose a human-readable parse error: which sub-call produced it, the
/// underlying serde error, and a bounded preview of the raw bytes the server
/// returned (often an HTML error page when the API misbehaves).
pub fn parse_err(cmd: &str, raw: &[u8], e: serde_json::Error) -> VastError {
    let preview_raw = String::from_utf8_lossy(raw);
    let trimmed = preview_raw.trim();
    let preview: String = if trimmed.is_empty() {
        "(empty stdout)".to_string()
    } else if trimmed.chars().count() > 200 {
        format!("{}…", trimmed.chars().take(200).collect::<String>())
    } else {
        trimmed.to_string()
    };
    VastError::ParseError(format!("vast {} → {} (raw: {})", cmd, e, preview))
}

#[derive(Debug, Clone)]
pub struct OfferQuery {
    pub gpu_name: String,
    pub gpu_count: u32,
    pub gpu_ram_gte: Option<u32>,
    pub dph_lte: Option<f64>,
    pub region: Option<String>,
    pub inet_up_gte: Option<f64>,
    pub inet_down_gte: Option<f64>,
    pub cuda_gte: Option<f64>,
    pub reliability_gte: Option<f64>,
    pub direct_port_count_gte: Option<u32>,
}

impl OfferQuery {
    /// Render the query into a human-readable summary (used in dry-run output).
    pub fn render(&self) -> String {
        let name = self.gpu_name.replace(' ', "_");
        let mut parts = vec![
            format!("gpu_name={}", name),
            format!("num_gpus={}", self.gpu_count),
        ];
        if let Some(vram) = self.gpu_ram_gte {
            parts.push(format!("gpu_ram>={}", vram));
        }
        if let Some(dph) = self.dph_lte {
            parts.push(format!("dph_total<={:.4}", dph));
        }
        if let Some(region) = &self.region {
            parts.push(format!("datacenter_region={}", region));
        }
        if let Some(up) = self.inet_up_gte {
            parts.push(format!("inet_up>={:.1}", up));
        }
        if let Some(down) = self.inet_down_gte {
            parts.push(format!("inet_down>={:.1}", down));
        }
        if let Some(cuda) = self.cuda_gte {
            parts.push(format!("cuda_max_good>={:.1}", cuda));
        }
        if let Some(rel) = self.reliability_gte {
            parts.push(format!("reliability2>={:.2}", rel));
        }
        if let Some(ports) = self.direct_port_count_gte {
            parts.push(format!("direct_port_count>={}", ports));
        }
        parts.join(" ")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Offer {
    pub id: u64,
    pub gpu_name: String,
    pub num_gpus: u32,
    pub gpu_ram: f64,
    pub dph_total: f64,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub reliability2: Option<f64>,
    pub disk_space: Option<f64>,
    pub status: Option<String>,
    #[serde(default)]
    pub geolocation: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UserInfo {
    pub id: Option<u64>,
    pub email: Option<String>,
    pub username: Option<String>,
    pub fullname: Option<String>,
    pub balance: Option<f64>,
    pub credit: Option<f64>,
    pub credit_balance: Option<f64>,
}

impl UserInfo {
    /// Pick the most "useful" balance figure. Vast's API has several
    /// near-synonymous fields depending on account type; prefer
    /// `credit` (current spendable), then `credit_balance`, then `balance`.
    pub fn effective_balance(&self) -> Option<f64> {
        self.credit.or(self.credit_balance).or(self.balance)
    }

    pub fn account_label(&self) -> Option<String> {
        self.email
            .clone()
            .or_else(|| self.username.clone())
            .or_else(|| self.fullname.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstanceInfo {
    pub id: u64,
    pub actual_status: Option<String>,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_key: Option<String>,
    pub gpu_name: Option<String>,
    pub dph_total: Option<f64>,
}

pub fn parse_user_info(raw: &[u8]) -> Result<UserInfo, VastError> {
    serde_json::from_slice::<UserInfo>(raw).map_err(|e| parse_err("show user", raw, e))
}
