#![deny(unsafe_code)]

use std::cell::RefCell;
use std::path::Path;
use std::sync::OnceLock;

use chrono::Utc;
use xrun_core::{
    config::credentials::VastCredentials,
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{InstanceCaps, NewEvent, RunId, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter, VendorRemoteInstance, VendorStatus},
};

use crate::{error::VastError, execute, provision, pull, stub::VastStub, tail, upload};

fn get_tokio_rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime for VastAdapter")
    })
}

pub struct VastAdapter {
    credentials: VastCredentials,
    store: RefCell<Store>,
    run_id: RefCell<Option<RunId>>,
    exclude_countries: Vec<String>,
    caps: RefCell<InstanceCaps>,
    upload_timeout: RefCell<Option<std::time::Duration>>,
}

impl VastAdapter {
    pub fn new(credentials: VastCredentials, store: Store) -> Self {
        Self::with_exclude_countries(credentials, store, Vec::new())
    }

    pub fn with_exclude_countries(
        credentials: VastCredentials,
        store: Store,
        exclude_countries: Vec<String>,
    ) -> Self {
        Self {
            credentials,
            store: RefCell::new(store),
            run_id: RefCell::new(None),
            exclude_countries,
            caps: RefCell::new(InstanceCaps::default()),
            upload_timeout: RefCell::new(None),
        }
    }

    /// Per-source upload deadline. `None` (default) means no timeout — we'd
    /// rather a 4 GB upload take 30 min on a 17 Mbps node than silently abort.
    /// Threaded from `manifest.policy.upload_timeout_secs`.
    pub fn set_upload_timeout(&self, dur: Option<std::time::Duration>) {
        *self.upload_timeout.borrow_mut() = dur;
    }

    /// Associate a run with this adapter so events/instances are linked.
    pub fn set_run_id(&self, run_id: &RunId) {
        *self.run_id.borrow_mut() = Some(run_id.clone());
    }

    /// Caps applied to instances provisioned by this adapter. Used by the
    /// poll-daemon to auto-destroy instances that exceed lifetime/cost/idle.
    pub fn set_caps(&self, caps: InstanceCaps) {
        *self.caps.borrow_mut() = caps;
    }

    async fn vendor_status_impl(&self) -> VendorStatus {
        let now = Utc::now();
        let Some(key) = self.credentials.api_key.clone() else {
            return VendorStatus {
                connected: false,
                balance: None,
                currency: None,
                account: None,
                last_checked: now,
                error: Some("api_key not set".to_string()),
            };
        };

        match crate::rest::show_user(&key).await {
            Ok(info) => VendorStatus {
                connected: true,
                balance: info.effective_balance(),
                currency: Some("USD".to_string()),
                account: info.account_label(),
                last_checked: now,
                error: None,
            },
            Err(e) => {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                let unauthorized = lower.contains("unauthor")
                    || lower.contains("requires login")
                    || msg.contains("401")
                    || msg.contains("403");
                VendorStatus {
                    connected: false,
                    balance: None,
                    currency: None,
                    account: None,
                    last_checked: now,
                    error: Some(if unauthorized {
                        "key rejected (revoked or expired). Get a new one at https://cloud.vast.ai/account/?tab=keys"
                            .to_string()
                    } else {
                        msg
                    }),
                }
            }
        }
    }

    async fn vendor_instances_impl(&self) -> Result<Vec<VendorRemoteInstance>, VastError> {
        let Some(key) = self.credentials.api_key.clone() else {
            return Ok(Vec::new());
        };
        let raw = crate::rest::show_instances(&key).await?;
        Ok(raw.into_iter().map(remote_to_generic).collect())
    }

    async fn provision_impl(&self, manifest: &Manifest) -> Result<InstanceHandle, VastError> {
        let vast = manifest
            .vast
            .as_ref()
            .ok_or_else(|| VastError::ParseError("vast section required".into()))?;

        let api_key = self
            .credentials
            .api_key
            .clone()
            .ok_or_else(|| VastError::CliFailure {
                exit_code: 401,
                stderr: "vast.api_key not set — run `xrun config set vast.api_key <KEY>` \
                         (or place the token in ~/.config/vastai/vast_api_key)"
                    .into(),
            })?;
        let query = provision::offer_query_from_manifest(vast);
        let body = crate::rest::build_offer_search_body(&query, 5.0);
        let body_str = serde_json::to_string(&body).unwrap_or_else(|_| "<unprintable>".to_string());
        let offers = crate::rest::search_offers(&api_key, &query).await?;
        let offers = provision::filter_excluded_countries(offers, &self.exclude_countries);

        let price_cap = vast.price.as_ref().map(|p| p.max_per_hour);
        let best = provision::rank_and_select(offers, price_cap, &body_str)?;
        let offer_id = best.id;
        let offer_dph = best.dph_total;

        let disk_gb = vast.disk_gb.unwrap_or(20);
        let ssh = vast.ssh.unwrap_or(true);
        let image = vast.image.clone();

        let instance_id =
            crate::rest::create_instance(&api_key, offer_id, &image, disk_gb, ssh).await?;

        // The instance is now billable. Any failure between here and a successful
        // return must auto-destroy it, otherwise we leak a paid GPU. `show_instance`
        // can return transient HTTP errors that previously left orphaned instances.
        let info = match wait_for_running(&api_key, instance_id).await {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!(
                    "provision failed after create_instance({instance_id}); auto-destroying: {e}"
                );
                if let Err(destroy_err) = crate::rest::destroy_instance(&api_key, instance_id).await
                {
                    tracing::error!(
                        "auto-destroy of {instance_id} failed (instance is leaking and will \
                         keep accruing cost!): {destroy_err}"
                    );
                }
                return Err(e);
            }
        };

        let ssh_host = info.ssh_host.clone();
        let ssh_port = info.ssh_port;
        let gpu_name = info.gpu_name.clone();
        let actual_dph = info.dph_total.unwrap_or(offer_dph);
        let now = Utc::now();

        // All async work done — do sync store writes without holding borrows across awaits.
        {
            let run_id_opt = self.run_id.borrow().clone();
            let mut store = self.store.borrow_mut();

            let caps = self.caps.borrow().clone();
            let _ = store.insert_instance_with_caps(
                &instance_id.to_string(),
                "vast",
                run_id_opt.as_ref(),
                gpu_name.as_deref(),
                Some(actual_dph),
                now,
                &caps,
            );

            if let Some(ref rid) = run_id_opt {
                let extra = serde_json::json!({ "offer_id": offer_id, "dph": actual_dph });
                let _ = store.append_event(
                    rid,
                    NewEvent {
                        ts: now,
                        stage: "provision".to_string(),
                        status: "ok".to_string(),
                        msg: None,
                        payload_json: Some(extra.to_string()),
                    },
                );
            }
        }

        Ok(InstanceHandle {
            id: instance_id.to_string(),
            vendor: "vast".to_string(),
            ssh_host,
            ssh_port,
            ssh_user: "root".to_string(),
        })
    }

    async fn upload_impl(
        &self,
        h: &InstanceHandle,
        sources: &[DataSource],
    ) -> Result<(), VastError> {
        let instance_id: u64 =
            h.id.parse()
                .map_err(|_| VastError::ParseError(format!("invalid instance id: {}", h.id)))?;

        // Emit start event, then do async upload, then emit ok event — never holding
        // the RefCell borrow across an await point.
        {
            let run_id_opt = self.run_id.borrow().clone();
            let mut store = self.store.borrow_mut();
            if let Some(ref rid) = run_id_opt {
                let _ = store.append_event(
                    rid,
                    NewEvent {
                        ts: Utc::now(),
                        stage: "upload".to_string(),
                        status: "start".to_string(),
                        msg: None,
                        payload_json: None,
                    },
                );
            }
        }

        let timeout = *self.upload_timeout.borrow();
        let result = upload::upload_sources(instance_id, h, sources, timeout).await;

        if let Err(ref e) = result {
            // Surface the cancellation cause as an event so `xrun events <id>`
            // shows *why* the run flipped, not just `instance_destroyed: ok`.
            let run_id_opt = self.run_id.borrow().clone();
            let mut store = self.store.borrow_mut();
            if let Some(ref rid) = run_id_opt {
                let (status, msg, payload) = match e {
                    VastError::UploadTimeout {
                        dst,
                        transferred,
                        elapsed_secs,
                        mbps,
                    } => (
                        "timeout".to_string(),
                        Some(format!(
                            "upload of {dst} timed out after {elapsed_secs}s ({transferred} bytes, {mbps:.1} Mbps)"
                        )),
                        Some(serde_json::json!({
                            "dst": dst,
                            "transferred": transferred,
                            "elapsed_secs": elapsed_secs,
                            "mbps": mbps,
                        }).to_string()),
                    ),
                    other => ("fail".to_string(), Some(other.to_string()), None),
                };
                let _ = store.append_event(
                    rid,
                    NewEvent {
                        ts: Utc::now(),
                        stage: "upload".to_string(),
                        status,
                        msg,
                        payload_json: payload,
                    },
                );
            }
        }
        result?;

        {
            let run_id_opt = self.run_id.borrow().clone();
            let mut store = self.store.borrow_mut();
            if let Some(ref rid) = run_id_opt {
                let _ = store.append_event(
                    rid,
                    NewEvent {
                        ts: Utc::now(),
                        stage: "upload".to_string(),
                        status: "ok".to_string(),
                        msg: None,
                        payload_json: None,
                    },
                );
            }
        }

        Ok(())
    }

    async fn execute_impl(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VastError> {
        // Sanity-check the id is parseable so we surface a nice error instead
        // of trying SSH against a bogus handle.
        let _instance_id: u64 =
            h.id.parse()
                .map_err(|_| VastError::ParseError(format!("invalid instance id: {}", h.id)))?;

        // Launch setup + background command over plain SSH (vast.ai's HTTP
        // execute endpoint rejects compound shell forms — see issue.md
        // Update 4). Borrows are dropped before each await.
        let pid = execute::launch_run(h, run_spec).await?;

        {
            let run_id_opt = self.run_id.borrow().clone();
            let mut store = self.store.borrow_mut();
            if let Some(ref rid) = run_id_opt {
                let _ = store.append_event(
                    rid,
                    NewEvent {
                        ts: Utc::now(),
                        stage: "env_ready".to_string(),
                        status: "ok".to_string(),
                        msg: None,
                        payload_json: None,
                    },
                );
                let _ = store.append_event(
                    rid,
                    NewEvent {
                        ts: Utc::now(),
                        stage: "train_start".to_string(),
                        status: "ok".to_string(),
                        msg: None,
                        payload_json: pid.map(|p| serde_json::json!({ "pid": p }).to_string()),
                    },
                );
            }
        }

        Ok(())
    }

    async fn tail_impl(
        &self,
        h: &InstanceHandle,
        file: &str,
        offset: u64,
    ) -> Result<Vec<u8>, VastError> {
        let host = h
            .ssh_host
            .as_deref()
            .ok_or_else(|| VastError::ParseError(format!("instance {} has no ssh_host", h.id)))?;
        let port = h
            .ssh_port
            .ok_or_else(|| VastError::ParseError(format!("instance {} has no ssh_port", h.id)))?;
        tail::tail_file(host, port, file, offset).await
    }

    async fn pull_impl(
        &self,
        h: &InstanceHandle,
        remote: &str,
        into: &Path,
    ) -> Result<(), VastError> {
        let instance_id: u64 =
            h.id.parse()
                .map_err(|_| VastError::ParseError(format!("invalid instance id: {}", h.id)))?;
        let host = h
            .ssh_host
            .as_deref()
            .ok_or_else(|| VastError::ParseError(format!("instance {} has no ssh_host", h.id)))?;
        let port = h
            .ssh_port
            .ok_or_else(|| VastError::ParseError(format!("instance {} has no ssh_port", h.id)))?;

        let pulled = pull::pull_files(host, port, instance_id, remote, into).await?;

        {
            let run_id_opt = self.run_id.borrow().clone();
            let mut store = self.store.borrow_mut();
            if let Some(ref rid) = run_id_opt {
                for file in &pulled {
                    let kind = pull::classify_kind(
                        file.local_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(""),
                    );
                    let _ = store.record_artifact(
                        rid,
                        xrun_core::store::NewArtifact {
                            kind,
                            remote_path: file.remote_path.clone(),
                            local_path: file.local_path.to_str().map(str::to_string),
                            size_bytes: file.size_bytes,
                            sha256: file.sha256.clone(),
                            is_best: false,
                        },
                    );
                }
            }
        }

        Ok(())
    }

    async fn destroy_impl(&self, h: &InstanceHandle) -> Result<(), VastError> {
        let instance_id: u64 =
            h.id.parse()
                .map_err(|_| VastError::ParseError(format!("invalid instance id: {}", h.id)))?;

        let key = self.credentials.api_key.clone().ok_or_else(|| {
            VastError::ParseError(
                "vast.api_key not configured — cannot destroy instance".to_string(),
            )
        })?;
        crate::rest::destroy_instance(&key, instance_id).await?;

        let now = Utc::now();

        {
            let run_id_opt = self.run_id.borrow().clone();
            let mut store = self.store.borrow_mut();

            let _ = store.update_instance_destroyed(&h.id, now);

            if let Some(ref rid) = run_id_opt {
                if let Ok(Some(run)) = store.get_run(rid) {
                    if let Some(started_at) = run.started_at {
                        if let Ok(Some(inst)) = store.get_instance(&h.id) {
                            if let Some(dph) = inst.price_per_hour {
                                let hours = (now - started_at).num_seconds().max(0) as f64 / 3600.0;
                                let _ = store.update_run_cost(rid, hours * dph);
                            }
                        }
                    }
                }

                let _ = store.append_event(
                    rid,
                    NewEvent {
                        ts: now,
                        stage: "instance_destroyed".to_string(),
                        status: "ok".to_string(),
                        msg: None,
                        payload_json: None,
                    },
                );
            }
        }

        Ok(())
    }
}

/// Poll `show_instance` until the instance reaches `running` or we hit the
/// 10-minute deadline. Extracted from `provision_impl` so the caller can wrap
/// it in an auto-destroy on error — see the call site comment for context.
async fn wait_for_running(
    api_key: &str,
    instance_id: u64,
) -> Result<crate::rest::RemoteInstance, VastError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        let info = crate::rest::show_instance(api_key, instance_id).await?;
        if info.actual_status.as_deref() == Some("running") {
            return Ok(info);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(VastError::InstanceLossOnProvision);
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

fn vast_to_vendor(e: VastError) -> VendorError {
    match e {
        VastError::FileTruncated { .. } => VendorError::Truncated,
        _ => VendorError::Other(e.to_string()),
    }
}

impl VendorAdapter for VastAdapter {
    fn name(&self) -> &'static str {
        "vast"
    }

    fn set_run_id(&self, run_id: &RunId) {
        *self.run_id.borrow_mut() = Some(run_id.clone());
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        VastStub::new().validate(manifest)
    }

    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError> {
        VastStub::new().dry_run_plan(manifest)
    }

    fn vendor_status(&self) -> Result<VendorStatus, VendorError> {
        Ok(get_tokio_rt().block_on(self.vendor_status_impl()))
    }

    fn vendor_instances(&self) -> Result<Vec<VendorRemoteInstance>, VendorError> {
        get_tokio_rt()
            .block_on(self.vendor_instances_impl())
            .map_err(vast_to_vendor)
    }

    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        get_tokio_rt()
            .block_on(self.provision_impl(manifest))
            .map_err(vast_to_vendor)
    }

    fn upload(&self, h: &InstanceHandle, sources: &[DataSource]) -> Result<(), VendorError> {
        get_tokio_rt()
            .block_on(self.upload_impl(h, sources))
            .map_err(vast_to_vendor)
    }

    fn execute(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError> {
        get_tokio_rt()
            .block_on(self.execute_impl(h, run_spec))
            .map_err(vast_to_vendor)
    }

    fn tail(&self, h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError> {
        get_tokio_rt()
            .block_on(self.tail_impl(h, file, offset))
            .map_err(vast_to_vendor)
    }

    fn pull(&self, h: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError> {
        get_tokio_rt()
            .block_on(self.pull_impl(h, remote, into))
            .map_err(vast_to_vendor)
    }

    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        get_tokio_rt()
            .block_on(self.destroy_impl(h))
            .map_err(vast_to_vendor)
    }

    fn process_alive(&self, h: &InstanceHandle) -> Option<bool> {
        // `cat run.pid` over ssh, then `kill -0 $PID`. If the PID file is
        // missing the run hasn't reached `train_start` yet — return None to
        // mean "no opinion" rather than `Some(false)`, otherwise the poller
        // would mark a still-provisioning run as failed.
        let host = h.ssh_host.as_deref()?;
        let port = h.ssh_port?;
        let probe = "if [ -f /workspace/run/run.pid ]; then \
                     PID=$(cat /workspace/run/run.pid); \
                     if kill -0 \"$PID\" 2>/dev/null; then echo alive; \
                     else echo dead; fi; \
                     else echo no_pid; fi";
        let bytes = get_tokio_rt()
            .block_on(crate::transfer::ssh_exec(host, port, probe))
            .ok()?;
        match String::from_utf8_lossy(&bytes).trim() {
            "alive" => Some(true),
            "dead" => Some(false),
            _ => None,
        }
    }
}

fn remote_to_generic(r: crate::rest::RemoteInstance) -> VendorRemoteInstance {
    let ssh = match (r.ssh_host.as_deref(), r.ssh_port) {
        (Some(host), Some(port)) if !host.is_empty() => Some(format!("{}:{}", host, port)),
        _ => None,
    };
    VendorRemoteInstance {
        id: r.id.to_string(),
        gpu: r.gpu_name,
        num_gpus: r.num_gpus,
        dph_total: r.dph_total,
        status: r.actual_status.or(r.cur_state),
        uptime_secs: r.duration.map(|d| d.max(0.0) as u64),
        ssh,
        region: r.geolocation,
    }
}
