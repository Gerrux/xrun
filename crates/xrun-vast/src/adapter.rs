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

use crate::{cli, error::VastError, execute, provision, pull, stub::VastStub, tail, upload};

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
        // Push the api key into the process-wide override so every subsequent
        // `vastai` invocation receives `--api-key …` and doesn't fall back to
        // the native ~/.config/vastai/vast_api_key file.
        crate::process::set_api_key_override(credentials.api_key.clone());
        Self {
            credentials,
            store: RefCell::new(store),
            run_id: RefCell::new(None),
            exclude_countries,
            caps: RefCell::new(InstanceCaps::default()),
        }
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

        crate::process::set_api_key_override(Some(key.clone()));
        // Hit the REST API directly: the `vastai` Python CLI returns 403 on
        // auth-required endpoints for some recent server versions even with a
        // valid key. REST works in both cases.
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

        let api_key = self.credentials.api_key.clone().ok_or_else(|| {
            VastError::CliFailure {
                exit_code: 401,
                stderr: "vast.api_key not set — run `xrun config set vast.api_key <KEY>` \
                         (or place the token in ~/.config/vastai/vast_api_key)"
                    .into(),
            }
        })?;
        crate::process::set_api_key_override(Some(api_key.clone()));

        let query = provision::offer_query_from_manifest(vast);
        let body = crate::rest::build_offer_search_body(&query, 5.0);
        let body_str =
            serde_json::to_string(&body).unwrap_or_else(|_| "<unprintable>".to_string());
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

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
        let info = loop {
            let info = crate::rest::show_instance(&api_key, instance_id).await?;
            if info.actual_status.as_deref() == Some("running") {
                break info;
            }
            if tokio::time::Instant::now() > deadline {
                return Err(VastError::InstanceLossOnProvision);
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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

        upload::upload_sources(instance_id, h, sources).await?;

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
        let instance_id: u64 =
            h.id.parse()
                .map_err(|_| VastError::ParseError(format!("invalid instance id: {}", h.id)))?;
        tail::tail_file(instance_id, file, offset).await
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

        let pulled = pull::pull_files(instance_id, remote, into).await?;

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

        if let Some(key) = self.credentials.api_key.clone() {
            crate::rest::destroy_instance(&key, instance_id).await?;
        } else {
            cli::destroy(instance_id).await?;
        }

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
