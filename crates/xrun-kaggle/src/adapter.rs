#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use xrun_core::{
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{RunId, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};

use crate::cli::{KaggleCli, KaggleProcess, KernelState};
use crate::embed;
use crate::error::KaggleError;
use crate::ingest::ingest_post_run;
use crate::kernel_metadata::KernelMetadata;

/// Wrapper script injected as `main.py` for script-mode kernels.
pub const XRUN_KAGGLE_ENTRY_PY: &str = include_str!("../tests/data/_xrun_kaggle_entry.py");

pub struct KaggleAdapter {
    cli: KaggleCli,
    run_id: std::sync::RwLock<Option<RunId>>,
    store_path: Option<PathBuf>,
}

impl KaggleAdapter {
    pub fn new() -> Self {
        Self {
            cli: KaggleCli::new(),
            run_id: std::sync::RwLock::new(None),
            store_path: None,
        }
    }

    pub fn with_process(process: Box<dyn KaggleProcess>) -> Self {
        Self {
            cli: KaggleCli::with_process(process),
            run_id: std::sync::RwLock::new(None),
            store_path: None,
        }
    }

    pub fn with_store_path(mut self, path: PathBuf) -> Self {
        self.store_path = Some(path);
        self
    }

    fn get_run_id(&self) -> Option<RunId> {
        self.run_id.read().ok().and_then(|g| g.clone())
    }
}

impl Default for KaggleAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn kaggle_err(e: KaggleError) -> VendorError {
    VendorError::Other(e.to_string())
}

impl VendorAdapter for KaggleAdapter {
    fn name(&self) -> &'static str {
        "kaggle"
    }

    fn set_run_id(&self, run_id: &RunId) {
        if let Ok(mut guard) = self.run_id.write() {
            *guard = Some(run_id.clone());
        }
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        let kaggle = manifest.kaggle.as_ref().ok_or_else(|| {
            VendorError::Validation("vendor=kaggle requires a [kaggle] section".to_string())
        })?;

        if kaggle.kernel_slug.is_empty() {
            return Err(VendorError::Validation(
                "kaggle.kernel_slug must not be empty".to_string(),
            ));
        }

        if !kaggle.kernel_slug.contains('/') {
            return Err(VendorError::Validation(
                "kaggle.kernel_slug must be in format <username>/<slug>".to_string(),
            ));
        }

        if manifest.run.cmd.is_none() && manifest.run.notebook.is_none() {
            return Err(VendorError::Validation(
                "Kaggle manifest requires either run.cmd or run.notebook".to_string(),
            ));
        }

        Ok(())
    }

    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError> {
        let kaggle = manifest
            .kaggle
            .as_ref()
            .ok_or_else(|| VendorError::Validation("no kaggle section".to_string()))?;

        Ok(DryRunPlan {
            gpu_query: format!(
                "kaggle:{}{}",
                kaggle.kernel_slug,
                if kaggle.enable_gpu.unwrap_or(false) {
                    " (GPU)"
                } else {
                    ""
                }
            ),
            estimated_price_max: 0.0, // Kaggle is free
            data_total_bytes: 0,
            data_items: vec![],
            cmd_line: manifest
                .run
                .cmd
                .clone()
                .unwrap_or_else(|| manifest.run.notebook.clone().unwrap_or_default()),
        })
    }

    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        let kaggle = manifest
            .kaggle
            .as_ref()
            .ok_or_else(|| VendorError::Validation("no kaggle section".to_string()))?;

        let staging_dir = tempfile::TempDir::new()
            .map_err(|e| VendorError::Other(format!("failed to create staging dir: {e}")))?;
        let staging = staging_dir.path();

        let enable_gpu = kaggle.enable_gpu.unwrap_or(false);
        let enable_internet = kaggle.enable_internet.unwrap_or(false);
        let is_notebook = manifest.run.notebook.is_some();

        let (_code_file, metadata) = if is_notebook {
            let nb_path = manifest.run.notebook.as_ref().unwrap();
            let nb_name = Path::new(nb_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("notebook.ipynb");
            // Copy the notebook
            std::fs::copy(nb_path, staging.join(nb_name))
                .map_err(|e| VendorError::Other(format!("failed to copy notebook: {e}")))?;
            (
                nb_name.to_string(),
                KernelMetadata::new_notebook(
                    &kaggle.kernel_slug,
                    &manifest.name,
                    nb_name,
                    enable_gpu,
                    enable_internet,
                ),
            )
        } else {
            // Script mode: write wrapper entry point
            std::fs::write(staging.join("main.py"), XRUN_KAGGLE_ENTRY_PY)
                .map_err(|e| VendorError::Other(format!("failed to write entry script: {e}")))?;
            (
                "main.py".to_string(),
                KernelMetadata::new_script(
                    &kaggle.kernel_slug,
                    &manifest.name,
                    "main.py",
                    enable_gpu,
                    enable_internet,
                ),
            )
        };

        // Copy data files
        if let Some(sources) = &manifest.data {
            for src in sources {
                let src_path = Path::new(&src.src);
                if src_path.exists() {
                    let dst_name = src_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !dst_name.is_empty() {
                        std::fs::copy(src_path, staging.join(dst_name))
                            .map_err(|e| VendorError::Other(format!("failed to copy data: {e}")))?;
                    }
                }
            }
        }

        // Embed wheel if available
        if !embed::XRUN_HOOK_WHEEL.is_empty() {
            std::fs::write(
                staging.join("xrun_hook-latest-py3-none-any.whl"),
                embed::XRUN_HOOK_WHEEL,
            )
            .map_err(|e| VendorError::Other(format!("failed to write wheel: {e}")))?;
        } else {
            tracing::warn!(
                "xrun_hook wheel not embedded — Kaggle kernel will run without xrun_hook"
            );
        }

        // Write kernel-metadata.json
        let meta_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| VendorError::Other(format!("failed to serialize metadata: {e}")))?;
        std::fs::write(staging.join("kernel-metadata.json"), &meta_json).map_err(|e| {
            VendorError::Other(format!("failed to write kernel-metadata.json: {e}"))
        })?;

        // Push!
        let slug = self.cli.push(staging).map_err(kaggle_err)?;

        // staging_dir is dropped (cleaned up) after push — that's fine
        drop(staging_dir);

        Ok(InstanceHandle {
            id: format!("kaggle:{slug}"),
            vendor: "kaggle".to_string(),
            ssh_host: None,
            ssh_port: None,
            ssh_user: "kaggle".to_string(),
        })
    }

    fn upload(&self, _h: &InstanceHandle, _sources: &[DataSource]) -> Result<(), VendorError> {
        // No-op: everything was pushed in provision()
        Ok(())
    }

    fn execute(&self, _h: &InstanceHandle, _run_spec: &RunSpec) -> Result<(), VendorError> {
        // No-op: push() already starts the kernel
        Ok(())
    }

    fn tail(&self, _h: &InstanceHandle, _file: &str, _offset: u64) -> Result<Vec<u8>, VendorError> {
        // Kaggle doesn't support live tail — always return empty
        Ok(vec![])
    }

    fn pull(&self, h: &InstanceHandle, _remote: &str, into: &Path) -> Result<(), VendorError> {
        let slug = h.id.strip_prefix("kaggle:").unwrap_or(&h.id);
        self.cli.output(slug, into).map_err(kaggle_err)?;

        // Ingest events + metrics from the downloaded output
        if let Some(run_id) = self.get_run_id() {
            if let Some(store_path) = &self.store_path {
                match Store::open(store_path) {
                    Ok(mut store) => match ingest_post_run(into, &mut store, &run_id) {
                        Ok((ev, m)) => {
                            tracing::info!("kaggle ingest: {ev} events, {m} metrics");
                        }
                        Err(e) => {
                            tracing::warn!("kaggle ingest failed: {e}");
                        }
                    },
                    Err(e) => {
                        tracing::warn!("could not open store for kaggle ingest: {e}");
                    }
                }
            }
        }

        Ok(())
    }

    fn destroy(&self, _h: &InstanceHandle) -> Result<(), VendorError> {
        // No-op: Kaggle auto-terminates kernels
        Ok(())
    }
}

/// Poll Kaggle kernel status until completion. Returns final RunId status.
pub fn poll_until_complete(
    adapter: &KaggleAdapter,
    handle: &InstanceHandle,
    into_dir: &Path,
    poll_interval: Duration,
) -> Result<xrun_core::store::RunStatus, VendorError> {
    use xrun_core::store::RunStatus;

    let slug = handle.id.strip_prefix("kaggle:").unwrap_or(&handle.id);

    loop {
        let status = adapter.cli.status(slug).map_err(kaggle_err)?;

        match status.status {
            KernelState::Complete => {
                adapter.pull(handle, "", into_dir)?;
                return Ok(RunStatus::Done);
            }
            KernelState::Error => {
                let msg = status
                    .error_message
                    .unwrap_or_else(|| "unknown kernel error".to_string());
                tracing::error!("kaggle kernel failed: {msg}");
                return Ok(RunStatus::Failed);
            }
            KernelState::Queued | KernelState::Running | KernelState::Unknown => {
                tracing::debug!(
                    state = ?status.status,
                    "kaggle kernel still running, sleeping {:?}",
                    poll_interval
                );
                std::thread::sleep(poll_interval);
            }
        }
    }
}
