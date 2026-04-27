#![deny(unsafe_code)]

use std::cell::RefCell;
use std::path::Path;

use chrono::Utc;
use xrun_core::{
    config::credentials::VastCredentials,
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{NewEvent, RunId, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};

use crate::{cli, error::VastError, execute, provision, stub::VastStub, upload};

pub struct VastAdapter {
    #[allow(dead_code)]
    credentials: VastCredentials,
    store: RefCell<Store>,
    run_id: RefCell<Option<RunId>>,
}

impl VastAdapter {
    pub fn new(credentials: VastCredentials, store: Store) -> Self {
        Self {
            credentials,
            store: RefCell::new(store),
            run_id: RefCell::new(None),
        }
    }

    /// Associate a run with this adapter so events/instances are linked.
    pub fn set_run_id(&self, run_id: RunId) {
        *self.run_id.borrow_mut() = Some(run_id);
    }

    async fn provision_impl(&self, manifest: &Manifest) -> Result<InstanceHandle, VastError> {
        let vast = manifest
            .vast
            .as_ref()
            .ok_or_else(|| VastError::ParseError("vast section required".into()))?;

        let query = provision::offer_query_from_manifest(vast);
        let offers = cli::search_offers(&query).await?;

        let price_cap = vast.price.as_ref().map(|p| p.max_per_hour);
        let best = provision::rank_and_select(offers, price_cap)?;
        let offer_id = best.id;
        let offer_dph = best.dph_total;

        let disk_gb = vast.disk_gb.unwrap_or(20);
        let ssh = vast.ssh.unwrap_or(true);
        let image = vast.image.clone();

        let instance_id = cli::create_instance(offer_id, &image, disk_gb, ssh).await?;

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
        let info = loop {
            let info = cli::show_instance(instance_id).await?;
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

            let _ = store.insert_instance(
                &instance_id.to_string(),
                "vast",
                run_id_opt.as_ref(),
                gpu_name.as_deref(),
                Some(actual_dph),
                now,
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
        let instance_id: u64 =
            h.id.parse()
                .map_err(|_| VastError::ParseError(format!("invalid instance id: {}", h.id)))?;

        // Launch setup + background command.  Borrows are dropped before each await.
        let pid = execute::launch_run(instance_id, run_spec).await?;

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

    async fn destroy_impl(&self, h: &InstanceHandle) -> Result<(), VastError> {
        let instance_id: u64 =
            h.id.parse()
                .map_err(|_| VastError::ParseError(format!("invalid instance id: {}", h.id)))?;

        cli::destroy(instance_id).await?;

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
                                let hours = (now - started_at).num_seconds() as f64 / 3600.0;
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
    VendorError::Other(e.to_string())
}

impl VendorAdapter for VastAdapter {
    fn name(&self) -> &'static str {
        "vast"
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        VastStub::new().validate(manifest)
    }

    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError> {
        VastStub::new().dry_run_plan(manifest)
    }

    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.provision_impl(manifest))
        })
        .map_err(vast_to_vendor)
    }

    fn upload(&self, h: &InstanceHandle, sources: &[DataSource]) -> Result<(), VendorError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.upload_impl(h, sources))
        })
        .map_err(vast_to_vendor)
    }

    fn execute(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.execute_impl(h, run_spec))
        })
        .map_err(vast_to_vendor)
    }

    fn tail(&self, _h: &InstanceHandle, _file: &str, _offset: u64) -> Result<Vec<u8>, VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn pull(&self, _h: &InstanceHandle, _remote: &str, _into: &Path) -> Result<(), VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.destroy_impl(h))
        })
        .map_err(vast_to_vendor)
    }
}
