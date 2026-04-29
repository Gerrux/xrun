#![deny(unsafe_code)]

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::VastError;
use crate::process::{run_vastai, run_vastai_with_retry, RetryPolicy};

/// Build a readable parse error for vastai output: includes which sub-command
/// produced it, the underlying serde error, and a bounded preview of the raw
/// bytes so the user can see what `vastai` actually returned (often an HTML
/// error page, an "owner: Extra inputs are not permitted" string, etc.).
fn parse_err(cmd: &str, raw: &[u8], e: serde_json::Error) -> VastError {
    let preview_raw = String::from_utf8_lossy(raw);
    let trimmed = preview_raw.trim();
    let preview: String = if trimmed.is_empty() {
        "(empty stdout)".to_string()
    } else if trimmed.chars().count() > 200 {
        format!("{}…", trimmed.chars().take(200).collect::<String>())
    } else {
        trimmed.to_string()
    };
    VastError::ParseError(format!("vastai {} → {} (raw: {})", cmd, e, preview))
}

pub type InstanceId = u64;

#[derive(Debug, Clone)]
pub struct OfferQuery {
    pub gpu_name: String,
    pub gpu_count: u32,
    pub gpu_ram_gte: Option<u32>,
    pub dph_lte: Option<f64>,
    pub region: Option<String>,
    pub inet_up_gte: Option<f64>,
}

impl OfferQuery {
    /// Render the query into a single vastai search condition string.
    /// Example: "gpu_name=RTX_4090 num_gpus=1 gpu_ram>=24"
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

/// Endpoint for `vastai copy`: either a local path or a remote instance path.
#[derive(Debug, Clone)]
pub enum CopyEndpoint {
    Local(PathBuf),
    Remote { instance: InstanceId, path: String },
}

impl CopyEndpoint {
    fn to_arg(&self) -> String {
        match self {
            CopyEndpoint::Local(p) => p.display().to_string(),
            CopyEndpoint::Remote { instance, path } => format!("{}:{}", instance, path),
        }
    }
}

fn idempotent_policy() -> RetryPolicy {
    RetryPolicy::default()
}

pub async fn show_user() -> Result<UserInfo, VastError> {
    let args = ["show", "user", "--raw"];
    let out = run_vastai_with_retry(&args, &idempotent_policy()).await?;
    parse_user_info(&out)
}

pub fn parse_user_info(raw: &[u8]) -> Result<UserInfo, VastError> {
    serde_json::from_slice::<UserInfo>(raw).map_err(|e| parse_err("show user", raw, e))
}

pub async fn search_offers(query: &OfferQuery) -> Result<Vec<Offer>, VastError> {
    let q = query.render();
    let args = ["search", "offers", "--raw", &q];
    let out = run_vastai_with_retry(&args, &idempotent_policy()).await?;
    serde_json::from_slice::<Vec<Offer>>(&out).map_err(|e| parse_err("search offers", &out, e))
}

pub async fn create_instance(
    offer_id: InstanceId,
    image: &str,
    disk_gb: u32,
    ssh: bool,
) -> Result<InstanceId, VastError> {
    let offer_id_str = offer_id.to_string();
    let disk_str = disk_gb.to_string();
    let mut args = vec![
        "create",
        "instance",
        &offer_id_str,
        "--image",
        image,
        "--disk",
        &disk_str,
    ];
    if ssh {
        args.push("--ssh");
    }
    // Non-idempotent: single attempt only.
    let out = run_vastai(&args).await?;
    let v: serde_json::Value =
        serde_json::from_slice(&out).map_err(|e| parse_err("create instance", &out, e))?;
    v["new_contract"].as_u64().ok_or_else(|| {
        let preview = String::from_utf8_lossy(&out).trim().to_string();
        VastError::ParseError(format!(
            "vastai create instance → missing new_contract (raw: {})",
            if preview.is_empty() {
                "(empty stdout)"
            } else {
                &preview
            }
        ))
    })
}

pub async fn show_instance(id: InstanceId) -> Result<InstanceInfo, VastError> {
    let id_str = id.to_string();
    let args = ["show", "instance", "--raw", &id_str];
    let out = run_vastai_with_retry(&args, &idempotent_policy()).await?;
    serde_json::from_slice::<InstanceInfo>(&out).map_err(|e| parse_err("show instance", &out, e))
}

pub async fn execute(id: InstanceId, cmd: &str) -> Result<Vec<u8>, VastError> {
    let id_str = id.to_string();
    let args = ["execute", &id_str, cmd];
    // Non-idempotent: single attempt only to avoid duplicate remote commands.
    run_vastai(&args).await
}

pub async fn copy(src: &CopyEndpoint, dst: &CopyEndpoint) -> Result<(), VastError> {
    let src_arg = src.to_arg();
    let dst_arg = dst.to_arg();
    let args = ["copy", &src_arg, &dst_arg];
    run_vastai_with_retry(&args, &idempotent_policy()).await?;
    Ok(())
}

pub async fn destroy(id: InstanceId) -> Result<(), VastError> {
    let id_str = id.to_string();
    let args = ["destroy", "instance", &id_str];
    // Non-idempotent: single attempt, but ignore "not found" errors gracefully.
    match run_vastai(&args).await {
        Ok(_) => Ok(()),
        Err(VastError::CliFailure { stderr, .. })
            if stderr.to_lowercase().contains("not found")
                || stderr.to_lowercase().contains("unknown instance") =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}
