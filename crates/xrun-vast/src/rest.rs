//! Direct HTTPS calls to the vast.ai REST API.
//!
//! The `vastai` Python CLI is unreliable for auth-required endpoints on some
//! recent server versions (returns 403 even with a valid key). For probes that
//! only need to read user state we hit the REST API directly.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli::{Offer, OfferQuery, UserInfo};
use crate::error::VastError;

/// Subset of fields returned by `GET /instances/` that we surface in the TUI.
/// All optional with `#[serde(default)]` so a schema change at vast doesn't
/// break parsing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RemoteInstance {
    pub id: u64,
    pub actual_status: Option<String>,
    pub cur_state: Option<String>,
    pub gpu_name: Option<String>,
    pub num_gpus: Option<u32>,
    pub dph_total: Option<f64>,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub image_uuid: Option<String>,
    pub geolocation: Option<String>,
    /// Seconds the container has been running (vast field).
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct InstancesEnvelope {
    instances: Vec<RemoteInstance>,
}

const DEFAULT_BASE_URL: &str = "https://console.vast.ai/api/v0";

fn client() -> Result<reqwest::Client, VastError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| VastError::ParseError(format!("http client: {}", e)))
}

/// GET /users/current/ — returns the same shape that `vastai show user --raw`
/// is supposed to return. Auth is `Authorization: Bearer <api_key>`.
pub async fn show_user(api_key: &str) -> Result<UserInfo, VastError> {
    show_user_at(DEFAULT_BASE_URL, api_key).await
}

pub async fn show_user_at(base_url: &str, api_key: &str) -> Result<UserInfo, VastError> {
    let url = format!("{}/users/current/", base_url.trim_end_matches('/'));
    let resp = client()?
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| VastError::ParseError(format!("http: {}", e)))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!("{}: unauthorized", status.as_u16()),
        });
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!("{}: {}", status.as_u16(), body),
        });
    }

    let body = resp
        .bytes()
        .await
        .map_err(|e| VastError::ParseError(format!("read body: {}", e)))?;
    crate::cli::parse_user_info(&body)
}

/// GET /instances/ — list the user's running/stopped instances.
pub async fn show_instances(api_key: &str) -> Result<Vec<RemoteInstance>, VastError> {
    show_instances_at(DEFAULT_BASE_URL, api_key).await
}

pub async fn show_instances_at(
    base_url: &str,
    api_key: &str,
) -> Result<Vec<RemoteInstance>, VastError> {
    let url = format!("{}/instances/", base_url.trim_end_matches('/'));
    let resp = client()?
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| VastError::ParseError(format!("http: {}", e)))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!("{}: unauthorized", status.as_u16()),
        });
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!("{}: {}", status.as_u16(), body),
        });
    }

    let env: InstancesEnvelope = resp
        .json()
        .await
        .map_err(|e| VastError::ParseError(format!("decode instances: {}", e)))?;
    Ok(env.instances)
}

/// Build the query body that vast.ai's `POST /bundles/` accepts. Mirrors the
/// shape used by the legacy `vastai search offers` CLI: each filter is a
/// `{op: value}` map, plus the meta keys `type`, `order`, `allocated_storage`.
pub fn build_offer_search_body(query: &OfferQuery, allocated_storage_gb: f64) -> Value {
    let mut q = serde_json::Map::new();
    // Defaults equivalent to the CLI: on-demand, verified, rentable, not rented.
    q.insert("verified".into(), json!({ "eq": true }));
    q.insert("external".into(), json!({ "eq": false }));
    q.insert("rentable".into(), json!({ "eq": true }));
    q.insert("rented".into(), json!({ "eq": false }));

    // The legacy `vastai search offers` CLI accepts `RTX_4090` (no spaces) on
    // the command line for shell-quoting reasons, but `parse_query` converts
    // underscores back to spaces *before* posting to the REST API. So the wire
    // form is `"RTX 4090"` (with a space). Both `"RTX 4090"` and `"RTX_4090"`
    // from the manifest must produce the same wire form here.
    q.insert(
        "gpu_name".into(),
        json!({ "eq": query.gpu_name.replace('_', " ") }),
    );
    q.insert("num_gpus".into(), json!({ "eq": query.gpu_count }));

    if let Some(vram) = query.gpu_ram_gte {
        q.insert("gpu_ram".into(), json!({ "gte": vram }));
    }
    if let Some(dph) = query.dph_lte {
        q.insert("dph_total".into(), json!({ "lte": dph }));
    }
    if let Some(region) = &query.region {
        q.insert("geolocation".into(), json!({ "eq": region }));
    }
    if let Some(up) = query.inet_up_gte {
        q.insert("inet_up".into(), json!({ "gte": up }));
    }
    if let Some(down) = query.inet_down_gte {
        q.insert("inet_down".into(), json!({ "gte": down }));
    }
    if let Some(cuda) = query.cuda_gte {
        q.insert("cuda_max_good".into(), json!({ "gte": cuda }));
    }
    if let Some(rel) = query.reliability_gte {
        q.insert("reliability2".into(), json!({ "gte": rel }));
    }
    if let Some(ports) = query.direct_port_count_gte {
        q.insert("direct_port_count".into(), json!({ "gte": ports }));
    }

    q.insert("type".into(), json!("on-demand"));
    q.insert("order".into(), json!([["score", "desc"]]));
    q.insert("allocated_storage".into(), json!(allocated_storage_gb));
    // Default `/bundles/` page is ~64 offers — bump it so the
    // country-exclusion filter and `rank_and_select` see the whole market,
    // not just the top of the score-sorted page.
    q.insert("limit".into(), json!(1024));

    Value::Object(q)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OffersEnvelope {
    offers: Vec<Offer>,
}

/// `POST /bundles/` — search for offers matching the query.
pub async fn search_offers(api_key: &str, query: &OfferQuery) -> Result<Vec<Offer>, VastError> {
    search_offers_at(DEFAULT_BASE_URL, api_key, query).await
}

pub async fn search_offers_at(
    base_url: &str,
    api_key: &str,
    query: &OfferQuery,
) -> Result<Vec<Offer>, VastError> {
    let url = format!("{}/bundles/", base_url.trim_end_matches('/'));
    let body = build_offer_search_body(query, 5.0);
    let resp = client()?
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| VastError::ParseError(format!("http search_offers: {}", e)))?;

    map_status(&resp.status())?;

    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(VastError::CliFailure {
            exit_code: code as i32,
            stderr: format!("REST POST /bundles/ → {}: {}", code, body),
        });
    }

    let env: OffersEnvelope = resp
        .json()
        .await
        .map_err(|e| VastError::ParseError(format!("decode offers: {}", e)))?;

    if env.offers.is_empty() {
        tracing::warn!(
            "vast: POST /bundles/ returned 0 offers for query: {}",
            serde_json::to_string(&body).unwrap_or_else(|_| "<unprintable>".to_string())
        );
    } else {
        tracing::debug!(
            "vast: POST /bundles/ returned {} offers for query: {}",
            env.offers.len(),
            serde_json::to_string(&body).unwrap_or_else(|_| "<unprintable>".to_string())
        );
    }

    Ok(env.offers)
}

/// `PUT /asks/{offer_id}/` — rent an offer. Mirrors the body the legacy CLI
/// sends for `vastai create instance --image … --disk … [--ssh]`.
pub async fn create_instance(
    api_key: &str,
    offer_id: u64,
    image: &str,
    disk_gb: u32,
    ssh: bool,
) -> Result<u64, VastError> {
    create_instance_at(DEFAULT_BASE_URL, api_key, offer_id, image, disk_gb, ssh).await
}

pub async fn create_instance_at(
    base_url: &str,
    api_key: &str,
    offer_id: u64,
    image: &str,
    disk_gb: u32,
    ssh: bool,
) -> Result<u64, VastError> {
    let url = format!("{}/asks/{}/", base_url.trim_end_matches('/'), offer_id);
    let body = json!({
        "client_id": "me",
        "image": image,
        "env": {},
        "price": null,
        "disk": disk_gb,
        "label": null,
        "extra": null,
        "onstart": null,
        "image_login": null,
        "python_utf8": false,
        "lang_utf8": false,
        "use_jupyter_lab": false,
        "jupyter_dir": null,
        "force": false,
        "cancel_unavail": false,
        "template_hash_id": null,
        "user": null,
        "runtype": if ssh { "ssh_proxy" } else { "args" },
    });

    let resp = client()?
        .put(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| VastError::ParseError(format!("http create_instance: {}", e)))?;

    map_status(&resp.status())?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!(
                "REST PUT /asks/{}/ → {}: {}",
                offer_id,
                status.as_u16(),
                text
            ),
        });
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| {
        VastError::ParseError(format!(
            "decode create_instance response ({}) — body: {}",
            e,
            preview(&text)
        ))
    })?;
    if let Some(success) = v.get("success").and_then(|s| s.as_bool()) {
        if !success {
            let msg = v
                .get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or("create returned success=false");
            return Err(VastError::CliFailure {
                exit_code: 0,
                stderr: format!("REST PUT /asks/{}/ → {}", offer_id, msg),
            });
        }
    }
    v.get("new_contract")
        .and_then(|n| n.as_u64())
        .ok_or_else(|| {
            VastError::ParseError(format!(
                "missing new_contract in create response — body: {}",
                preview(&text)
            ))
        })
}

/// `DELETE /instances/{id}/` — destroy an instance.
pub async fn destroy_instance(api_key: &str, id: u64) -> Result<(), VastError> {
    destroy_instance_at(DEFAULT_BASE_URL, api_key, id).await
}

pub async fn destroy_instance_at(base_url: &str, api_key: &str, id: u64) -> Result<(), VastError> {
    let url = format!("{}/instances/{}/", base_url.trim_end_matches('/'), id);
    let resp = client()?
        .delete(&url)
        .bearer_auth(api_key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| VastError::ParseError(format!("http destroy_instance: {}", e)))?;

    let status = resp.status();
    // Tolerate "already gone" — matches cli::destroy fallback semantics.
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    map_status(&status)?;
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let lower = body.to_lowercase();
        if lower.contains("not found") || lower.contains("unknown instance") {
            return Ok(());
        }
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!(
                "REST DELETE /instances/{}/ → {}: {}",
                id,
                status.as_u16(),
                body
            ),
        });
    }
    Ok(())
}

/// `GET /instances/{id}/` — single instance view, without the legacy
/// `?owner=me` query string that current vast.ai backends reject.
pub async fn show_instance(api_key: &str, id: u64) -> Result<RemoteInstance, VastError> {
    show_instance_at(DEFAULT_BASE_URL, api_key, id).await
}

pub async fn show_instance_at(
    base_url: &str,
    api_key: &str,
    id: u64,
) -> Result<RemoteInstance, VastError> {
    let url = format!("{}/instances/{}/", base_url.trim_end_matches('/'), id);
    let resp = client()?
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| VastError::ParseError(format!("http show_instance: {}", e)))?;

    map_status(&resp.status())?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!(
                "REST GET /instances/{}/ → {}: {}",
                id,
                status.as_u16(),
                text
            ),
        });
    }
    // The /instances/{id}/ endpoint returns either `{"instances": <row>}` or a
    // bare row depending on backend version. Accept both.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SingleResp {
        Wrapped { instances: RemoteInstance },
        Bare(RemoteInstance),
    }
    let parsed: SingleResp = serde_json::from_str(&text).map_err(|e| {
        VastError::ParseError(format!(
            "decode show_instance ({}) — body: {}",
            e,
            preview(&text)
        ))
    })?;
    Ok(match parsed {
        SingleResp::Wrapped { instances } => instances,
        SingleResp::Bare(r) => r,
    })
}

fn map_status(status: &reqwest::StatusCode) -> Result<(), VastError> {
    if *status == reqwest::StatusCode::UNAUTHORIZED || *status == reqwest::StatusCode::FORBIDDEN {
        return Err(VastError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!("{}: unauthorized", status.as_u16()),
        });
    }
    Ok(())
}

fn preview(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() > 200 {
        format!("{}…", t.chars().take(200).collect::<String>())
    } else {
        t.to_string()
    }
}
