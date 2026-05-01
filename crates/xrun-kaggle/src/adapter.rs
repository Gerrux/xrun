#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use xrun_core::{
    config::credentials::KaggleCredentials,
    error::VendorError,
    manifest::{DataSource, Manifest, RunSpec},
    store::{RunId, RunStatus, Store},
    vendor::{
        DryRunPlan, InstanceHandle, PollCompletion, SyntheticEvent, VendorAdapter,
        VendorRemoteInstance, VendorStatus,
    },
};
use xrun_mlflow::{Auth as MlflowAuth, MlflowClient};

use crate::cli::{KaggleCli, KaggleProcess, KernelState};
use crate::embed;
use crate::error::KaggleError;
use crate::http::{self, CancelOutcome, KaggleApiClient};
use crate::ingest::ingest_post_run;
use crate::kernel_metadata::KernelMetadata;
use crate::log_stream::{
    parse_chunk_seq, slice_from_offset, ARTIFACT_PREFIX, LOG_STREAM_EXPERIMENT,
    LOG_STREAM_FILE, TAG_RUN_ID,
};

/// Wrapper script injected as `main.py` for script-mode kernels.
pub const XRUN_KAGGLE_ENTRY_PY: &str = include_str!("../tests/data/_xrun_kaggle_entry.py");

/// MLflow tracking-server config used for the live-log side channel. When
/// present, `provision()` injects MLFLOW_* env vars into the kernel's main.py
/// so xrun_hook can stream stdout chunks, and `tail()` pulls those chunks back.
#[derive(Clone, Debug)]
pub struct MlflowConfig {
    pub url: String,
    pub auth: Option<MlflowAuth>,
}

pub struct KaggleAdapter {
    cli: KaggleCli,
    run_id: std::sync::RwLock<Option<RunId>>,
    store_path: Option<PathBuf>,
    /// Tracks the last observed kernel state to emit transition events exactly once.
    last_kernel_state: Mutex<Option<KernelState>>,
    /// Stored separately from the cli env vars so the HTTP client (used by
    /// `destroy`) can authenticate against the REST API without going through
    /// the kaggle subprocess.
    credentials: KaggleCredentials,
    /// MLflow tracking config for live-log streaming. None disables streaming
    /// and `tail()` returns empty (matches pre-MLflow behaviour).
    mlflow: Option<MlflowConfig>,
    /// Cached MLflow run_id + artifact storage prefix, resolved by
    /// `xrun_run_id` tag. Filled lazily on the first `tail()` call so a
    /// kernel that's slow to start doesn't make `set_run_id()` block.
    mlflow_run_cache: Mutex<Option<(String, String)>>,
}

impl KaggleAdapter {
    pub fn new() -> Self {
        Self {
            cli: KaggleCli::new(),
            run_id: std::sync::RwLock::new(None),
            store_path: None,
            last_kernel_state: Mutex::new(None),
            credentials: KaggleCredentials::default(),
            mlflow: None,
            mlflow_run_cache: Mutex::new(None),
        }
    }

    pub fn with_process(process: Box<dyn KaggleProcess>) -> Self {
        Self {
            cli: KaggleCli::with_process(process),
            run_id: std::sync::RwLock::new(None),
            store_path: None,
            last_kernel_state: Mutex::new(None),
            credentials: KaggleCredentials::default(),
            mlflow: None,
            mlflow_run_cache: Mutex::new(None),
        }
    }

    /// Configure live-log streaming via an MLflow tracking server. The URL
    /// gets baked into the kernel's main.py so xrun_hook can push log chunks
    /// from inside Kaggle; `tail()` then pulls them back.
    pub fn with_mlflow(mut self, url: String, auth: Option<MlflowAuth>) -> Self {
        self.mlflow = Some(MlflowConfig { url, auth });
        self
    }

    pub fn with_store_path(mut self, path: PathBuf) -> Self {
        self.store_path = Some(path);
        self
    }

    /// Inject Kaggle credentials so the kaggle CLI subprocess can authenticate.
    /// Sets `KAGGLE_USERNAME`/`KAGGLE_KEY` (legacy) or `KAGGLE_API_TOKEN` (new Bearer).
    pub fn with_credentials(mut self, creds: KaggleCredentials) -> Self {
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(token) = &creds.token {
            env.push(("KAGGLE_API_TOKEN".into(), token.clone()));
        } else if let (Some(user), Some(key)) = (&creds.username, &creds.key) {
            env.push(("KAGGLE_USERNAME".into(), user.clone()));
            env.push(("KAGGLE_KEY".into(), key.clone()));
        }
        if !env.is_empty() {
            self.cli = self.cli.with_env(env);
        }
        self.credentials = creds;
        self
    }

    /// Expose the underlying `KaggleCli` for dataset operations.
    pub fn cli(&self) -> &KaggleCli {
        &self.cli
    }

    fn get_run_id(&self) -> Option<RunId> {
        self.run_id.read().ok().and_then(|g| g.clone())
    }

    /// Derive the artifacts directory from store_path + run_id.
    fn artifacts_dir(&self) -> Option<PathBuf> {
        let run_id = self.get_run_id()?;
        let sp = self.store_path.as_ref()?;
        Some(sp.join("runs").join(run_id.to_string()).join("artifacts"))
    }

    /// Build the `os.environ['…'] = '…'` block prepended to the kernel's
    /// main.py. Sets the env vars xrun_hook's log streamer reads to activate
    /// itself. Returns an empty string when MLflow is not configured — the
    /// streamer then stays inert and `tail()` keeps returning empty.
    fn build_env_prelude(&self) -> String {
        let Some(cfg) = &self.mlflow else {
            return String::new();
        };
        let Some(run_id) = self.get_run_id() else {
            // Without an xrun run_id the poller can't search for the streamer's
            // MLflow run by tag, so streaming would be useless.
            return String::new();
        };

        let mut lines: Vec<String> = vec![
            format!(
                "os.environ['MLFLOW_TRACKING_URI'] = {}",
                py_str(&cfg.url)
            ),
            format!(
                "os.environ['XRUN_RUN_ID'] = {}",
                py_str(&run_id.to_string())
            ),
            format!(
                "os.environ['XRUN_LOG_STREAM_FILE'] = {}",
                py_str(LOG_STREAM_FILE)
            ),
            format!(
                "os.environ['XRUN_LOG_STREAM_EXPERIMENT'] = {}",
                py_str(LOG_STREAM_EXPERIMENT)
            ),
        ];
        match &cfg.auth {
            Some(MlflowAuth::Bearer(token)) => {
                lines.push(format!(
                    "os.environ['MLFLOW_TRACKING_TOKEN'] = {}",
                    py_str(token)
                ));
            }
            Some(MlflowAuth::Basic { username, password }) => {
                lines.push(format!(
                    "os.environ['MLFLOW_TRACKING_USERNAME'] = {}",
                    py_str(username)
                ));
                lines.push(format!(
                    "os.environ['MLFLOW_TRACKING_PASSWORD'] = {}",
                    py_str(password)
                ));
            }
            None => {}
        }
        // Trailing newline keeps the prelude from glueing onto the next stmt.
        format!("{}\n", lines.join("\n"))
    }

    /// Resolve the streamer's MLflow run_id and artifact storage prefix by
    /// `xrun_run_id` tag. Cached after the first successful lookup so
    /// subsequent `tail()` calls go straight to artifact listing.
    fn resolve_mlflow_run(
        &self,
        client: &MlflowClient,
        xrun_run_id: &str,
    ) -> Result<Option<(String, String)>, VendorError> {
        if let Ok(guard) = self.mlflow_run_cache.lock() {
            if let Some(pair) = guard.as_ref() {
                return Ok(Some(pair.clone()));
            }
        }

        let exp_id = match block_async(client.get_or_create_experiment(LOG_STREAM_EXPERIMENT)) {
            Ok(id) => id,
            Err(e) => {
                tracing::debug!("kaggle tail: MLflow experiment lookup failed: {e}");
                return Ok(None);
            }
        };
        let runs = match block_async(client.search_runs_by_tag(&exp_id, TAG_RUN_ID, xrun_run_id)) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("kaggle tail: MLflow runs/search failed: {e}");
                return Ok(None);
            }
        };
        // The streamer creates exactly one run per xrun launch; if there are
        // duplicates (re-run, partial failure) the most recent one wins.
        let mlflow_run_id = match runs.into_iter().next() {
            Some(id) => id,
            None => return Ok(None),
        };
        let artifact_path = match block_async(client.get_run_artifact_path(&mlflow_run_id)) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!("kaggle tail: artifact_uri lookup failed: {e}");
                return Ok(None);
            }
        };
        let pair = (mlflow_run_id, artifact_path);
        if let Ok(mut guard) = self.mlflow_run_cache.lock() {
            *guard = Some(pair.clone());
        }
        Ok(Some(pair))
    }
}

/// Quote a string as a Python string literal, escaping backslashes and
/// single quotes. Used for the env prelude.
fn py_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Block on a future regardless of whether we're already inside a tokio
/// runtime. The poller may or may not own one; the kaggle adapter has to
/// work in both cases.
fn block_async<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");
            rt.block_on(fut)
        }
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

        // Collect dataset slugs: legacy single `dataset` + new `datasets` list.
        let dataset_sources: Vec<String> = {
            let mut v: Vec<String> = kaggle.datasets.clone();
            if let Some(single) = &kaggle.dataset {
                if !v.contains(single) {
                    v.push(single.clone());
                }
            }
            v
        };

        // §8: Wait for each dataset to be ready before pushing the kernel.
        for slug in &dataset_sources {
            let timeout = Duration::from_secs(120);
            let started = std::time::Instant::now();
            loop {
                match self.cli.is_dataset_ready(slug) {
                    Ok(true) => break,
                    Ok(false) => {
                        if started.elapsed() > timeout {
                            return Err(VendorError::Other(format!(
                                "dataset '{slug}' not ready after 120s; \
                                 run `kaggle datasets status {slug}` to check"
                            )));
                        }
                        tracing::info!("waiting for dataset '{slug}' to be ready...");
                        std::thread::sleep(Duration::from_secs(5));
                    }
                    Err(e) => {
                        // Non-fatal: proceed without readiness guarantee
                        tracing::warn!("could not check dataset status for '{slug}': {e}");
                        break;
                    }
                }
            }
        }

        let (_code_file, metadata) = if is_notebook {
            let nb_path = manifest.run.notebook.as_ref().unwrap();
            let nb_name = Path::new(nb_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("notebook.ipynb");
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
                    dataset_sources.clone(),
                ),
            )
        } else {
            let setup_block = manifest
                .run
                .setup
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_string();
            let cmd_block = manifest.run.cmd.as_deref().unwrap_or("").trim().to_string();
            let workdir = manifest.run.workdir.as_deref().unwrap_or("/kaggle/working");
            // §11: pass dataset slugs so main.py can probe XRUN_INPUT_DIR
            let env_prelude = self.build_env_prelude();
            // Kaggle script-mode kernels only upload the single `code_file`,
            // dropping every sibling we put in staging. To get xrun_hook into
            // the kernel we base64-embed the wheel directly into main.py and
            // pip-install it before user setup runs.
            let wheel_b64 = if !embed::XRUN_HOOK_WHEEL.is_empty() {
                Some(base64_encode(embed::XRUN_HOOK_WHEEL))
            } else {
                None
            };
            let main_py = build_script_main(
                &env_prelude,
                wheel_b64.as_deref(),
                &setup_block,
                &cmd_block,
                workdir,
                &dataset_sources,
            );
            std::fs::write(staging.join("main.py"), main_py)
                .map_err(|e| VendorError::Other(format!("failed to write entry script: {e}")))?;
            (
                "main.py".to_string(),
                KernelMetadata::new_script(
                    &kaggle.kernel_slug,
                    &manifest.name,
                    "main.py",
                    enable_gpu,
                    enable_internet,
                    dataset_sources.clone(),
                ),
            )
        };

        // Copy data files — file sources only; directories are not supported for Kaggle
        if let Some(sources) = &manifest.data {
            for src in sources {
                let src_path = Path::new(&src.src);
                if !src_path.exists() {
                    continue;
                }
                if src_path.is_dir() {
                    tracing::warn!(
                        "Kaggle: skipping directory data source '{}' — \
                         directories cannot be bundled in a kernel push. \
                         Upload the data as a Kaggle dataset and reference it \
                         via `kaggle.datasets: [owner/slug]` in the manifest.",
                        src.src
                    );
                    continue;
                }
                let dst_name = src_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !dst_name.is_empty() {
                    std::fs::copy(src_path, staging.join(dst_name))
                        .map_err(|e| VendorError::Other(format!("failed to copy data: {e}")))?;
                }
            }
        }

        // Wheel is base64-embedded directly into main.py for script kernels
        // (script-mode strips sibling files). For notebook-mode the user is
        // expected to install xrun_hook via their notebook's own setup cells —
        // we still warn at build time when the wheel is missing.
        if embed::XRUN_HOOK_WHEEL.is_empty() {
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

        // §9: Push with 409-conflict retry
        let slug = push_with_retry(&self.cli, staging, 2)?;

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

    fn tail(&self, _h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError> {
        // Live tail is supported only for stdout via the MLflow side channel
        // populated by xrun_hook's log streamer. Events/metrics still arrive
        // post-completion via `pull()` + `ingest_post_run`.
        if !file.contains("stdout") {
            return Ok(Vec::new());
        }
        let Some(cfg) = &self.mlflow else {
            return Ok(Vec::new());
        };
        let Some(xrun_run_id) = self.get_run_id() else {
            return Ok(Vec::new());
        };

        let client = MlflowClient::new(cfg.url.clone(), cfg.auth.clone());
        let (_mlflow_run_id, artifact_path) =
            match self.resolve_mlflow_run(&client, &xrun_run_id.to_string())? {
                Some(pair) => pair,
                // Streamer hasn't created its run yet (kernel still warming
                // up). Returning empty keeps the poller's offset unchanged
                // so the next tick will retry.
                None => return Ok(Vec::new()),
            };

        let artifacts = match block_async(
            client.list_artifacts(&artifact_path, Some(ARTIFACT_PREFIX)),
        ) {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!("kaggle tail: artifacts/list failed: {e}");
                return Ok(Vec::new());
            }
        };

        // Sort by chunk seq so reassembly order matches the in-kernel write
        // order — even when the MLflow listing returns them out of order.
        let mut chunks: Vec<(u32, String, u64)> = artifacts
            .into_iter()
            .filter_map(|(p, sz)| parse_chunk_seq(&p).map(|seq| (seq, p, sz)))
            .collect();
        chunks.sort_by_key(|(seq, _, _)| *seq);

        slice_from_offset::<VendorError>(&chunks, offset, |idx| {
            let path = &chunks[idx].1;
            block_async(client.download_artifact(&artifact_path, path))
                .map_err(|e| VendorError::Other(format!("kaggle tail download failed: {e}")))
        })
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

                // §1: Copy stdout log so `xrun logs <id>` can read it
                let xrun_log = into.join("__xrun_stdout.log");
                if xrun_log.exists() {
                    let run_log = store_path
                        .join("runs")
                        .join(run_id.to_string())
                        .join("stdout.log");
                    if let Err(e) = std::fs::copy(&xrun_log, &run_log) {
                        tracing::warn!("failed to copy kaggle stdout log: {e}");
                    }
                }
            }
        }

        Ok(())
    }

    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        // §4: Cancel the running kernel via the Kaggle REST API.
        //
        // Kaggle CLI 1.8.x removed `kaggle kernels cancel` entirely (and
        // Kaggle's own PR #967 to add it back was closed un-merged), so we
        // POST directly to /api/v1/kernels/cancel-session/{id}. The two-step
        // dance (resolve session id → cancel) is unavoidable: `cancel-session`
        // takes the integer session id, not the slug.
        let slug = h.id.strip_prefix("kaggle:").unwrap_or(&h.id);

        let auth = match http::auth_from_credentials(&self.credentials) {
            Some(a) => a,
            None => {
                tracing::warn!(
                    "kaggle destroy: no credentials configured — cannot cancel kernel '{slug}'. \
                     Stop the kernel via https://www.kaggle.com/code or run \
                     `xrun config set kaggle.username/.key`."
                );
                return Ok(());
            }
        };

        let client = match KaggleApiClient::new(auth) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("kaggle destroy: HTTP client init failed: {e}");
                return Ok(());
            }
        };

        match client.cancel_kernel(slug) {
            Ok(CancelOutcome::Cancelled(id)) => {
                tracing::info!("kaggle kernel '{slug}' cancelled (session {id})");
            }
            Ok(CancelOutcome::NoActiveSession) => {
                tracing::info!("kaggle kernel '{slug}' already finished — nothing to cancel");
            }
            Err(e) => {
                // Don't fail the stop: the run is marked Cancelled in the
                // local DB either way. A surfaced warn is more useful than
                // an opaque exit 1.
                tracing::warn!(
                    "could not cancel kaggle kernel '{slug}' via REST: {e}\n\
                     The kernel will auto-terminate after its session limit (≤12 h). \
                     Local run marked as stopped."
                );
            }
        }
        Ok(())
    }

    // §5: vendor_status() — confirm credentials and return account name
    fn vendor_status(&self) -> Result<VendorStatus, VendorError> {
        match self.cli.username() {
            Ok(account) => Ok(VendorStatus {
                connected: true,
                balance: None,
                currency: None,
                account: Some(account),
                last_checked: Utc::now(),
                error: Some("GPU quota: 30 h/week (not queryable via API)".to_string()),
            }),
            Err(e) => Ok(VendorStatus {
                connected: false,
                balance: None,
                currency: None,
                account: None,
                last_checked: Utc::now(),
                error: Some(e.to_string()),
            }),
        }
    }

    // §6: vendor_instances() — list running/queued kernels
    fn vendor_instances(&self) -> Result<Vec<VendorRemoteInstance>, VendorError> {
        let items = self.cli.list_mine().map_err(kaggle_err)?;
        let instances = items
            .into_iter()
            .filter(|item| {
                matches!(
                    item.status.as_deref().map(|s| s.to_lowercase()).as_deref(),
                    Some("running") | Some("queued")
                )
            })
            .map(|item| VendorRemoteInstance {
                id: item.slug_ref.clone(),
                gpu: None,
                num_gpus: None,
                dph_total: None,
                status: item.status,
                uptime_secs: item.run_seconds,
                ssh: None,
                region: None,
            })
            .collect();
        Ok(instances)
    }

    // §2: poll_completion() — emit synthetic lifecycle events and detect kernel termination
    fn poll_completion(&self, h: &InstanceHandle, _run_dir: &Path) -> Option<PollCompletion> {
        let slug = h.id.strip_prefix("kaggle:").unwrap_or(&h.id);
        let status = match self.cli.status(slug) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("kaggle poll_completion: status check failed: {e}");
                return None;
            }
        };
        tracing::debug!(
            "kaggle poll_completion {slug} → state={:?} msg={:?}",
            status.status,
            status.error_message
        );

        let mut last_guard = self
            .last_kernel_state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = last_guard.clone();
        let new_state = status.status.clone();

        let mut events: Vec<SyntheticEvent> = Vec::new();

        // Emit transition events (emitted exactly once per transition)
        if prev.as_ref() != Some(&KernelState::Queued) && new_state == KernelState::Queued {
            events.push(SyntheticEvent {
                stage: "queued".into(),
                status: "start".into(),
                msg: None,
            });
        }
        if prev.as_ref() != Some(&KernelState::Running) && new_state == KernelState::Running {
            events.push(SyntheticEvent {
                stage: "running".into(),
                status: "start".into(),
                msg: None,
            });
        }

        *last_guard = Some(new_state.clone());
        drop(last_guard);

        match new_state {
            KernelState::Complete => {
                // Pull artifacts before signalling Done
                if let Some(artifacts_dir) = self.artifacts_dir() {
                    if let Err(e) = std::fs::create_dir_all(&artifacts_dir) {
                        tracing::warn!("could not create artifacts dir: {e}");
                    } else if let Err(e) = self.pull(h, "", &artifacts_dir) {
                        tracing::warn!("kaggle poll_completion pull failed: {e}");
                    }
                }
                events.push(SyntheticEvent {
                    stage: "done".into(),
                    status: "ok".into(),
                    msg: None,
                });
                Some(PollCompletion {
                    terminal_status: Some(RunStatus::Done),
                    events,
                })
            }
            KernelState::Error => {
                let msg = status.error_message;
                events.push(SyntheticEvent {
                    stage: "done".into(),
                    status: "fail".into(),
                    msg,
                });
                Some(PollCompletion {
                    terminal_status: Some(RunStatus::Failed),
                    events,
                })
            }
            KernelState::Queued | KernelState::Running | KernelState::Unknown => {
                if events.is_empty() {
                    None
                } else {
                    Some(PollCompletion {
                        terminal_status: None,
                        events,
                    })
                }
            }
        }
    }
}

/// Push the kernel with up to `max_retries` retries on 409 Conflict.
///
/// When the previous kernel version is still running, Kaggle returns 409. We
/// poll until the current version completes, then retry the push once.
fn push_with_retry(
    cli: &KaggleCli,
    staging: &Path,
    max_retries: u32,
) -> Result<String, VendorError> {
    for attempt in 0..=max_retries {
        match cli.push(staging) {
            Ok(slug) => return Ok(slug),
            Err(KaggleError::CliFailure { ref stderr, .. }) if stderr.contains("409") => {
                if attempt >= max_retries {
                    return Err(VendorError::Other(format!(
                        "kaggle push failed after {max_retries} retries: previous kernel \
                         version still running or queued. Try again later."
                    )));
                }
                tracing::warn!(
                    "kaggle push returned 409 — previous version still active. \
                     Waiting 30s before retry ({}/{max_retries})…",
                    attempt + 1
                );
                std::thread::sleep(Duration::from_secs(30));
            }
            Err(e) => return Err(kaggle_err(e)),
        }
    }
    unreachable!()
}

/// Generate the `main.py` script that Kaggle will run.
///
/// §11: Probes both old and new dataset mount paths and sets `XRUN_INPUT_DIR`.
/// §1: Streams subprocess output to both stdout and `__xrun_stdout.log`.
///
/// `wheel_b64` is the base64-encoded xrun_hook wheel, embedded directly into
/// main.py because Kaggle script-mode strips sibling files from kernel pushes.
/// When None, the wheel-bootstrap block is omitted entirely.
fn build_script_main(
    env_prelude: &str,
    wheel_b64: Option<&str>,
    setup: &str,
    cmd: &str,
    workdir: &str,
    datasets: &[String],
) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace("'''", r"\'\'\'");
    let setup_escaped = escape(setup);
    let cmd_escaped = escape(cmd);
    let workdir_repr = format!("'{}'", workdir.replace('\'', "\\'"));

    // Build a Python list literal of dataset slugs
    let datasets_repr = format!(
        "[{}]",
        datasets
            .iter()
            .map(|s| format!("'{}'", s.replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Wheel-bootstrap block: decode base64 → write /tmp file → pip install →
    // import. Runs once after env setup so xrun_hook's log streamer starts
    // before _run() emits anything (we still capture later writes via the
    // tailed log file, but starting early keeps the streamer-run creation
    // off the critical path of the user's first command).
    let wheel_block = match wheel_b64 {
        Some(b64) => format!(
            "_WHEEL_B64 = '''{b64}'''\n\
             import base64 as _b64, tempfile as _tf, subprocess as _sp\n\
             # pip rejects wheels whose filename doesn't match the\n\
             # `<pkg>-<ver>-<py>-<abi>-<plat>.whl` convention, so write into\n\
             # a temp dir under the canonical name rather than mktemp's random one.\n\
             _whl_dir = _tf.mkdtemp(prefix='xrun_hook_')\n\
             _whl_path = os.path.join(_whl_dir, 'xrun_hook-0.0.0-py3-none-any.whl')\n\
             with open(_whl_path, 'wb') as _f:\n\
             \x20\x20\x20\x20_f.write(_b64.b64decode(_WHEEL_B64))\n\
             _r = _sp.run([sys.executable, '-m', 'pip', 'install', '--quiet', \
             '--no-deps', _whl_path], capture_output=True, text=True)\n\
             if _r.returncode != 0:\n\
             \x20\x20\x20\x20print('xrun_hook bootstrap failed:', _r.stderr, flush=True)\n\
             else:\n\
             \x20\x20\x20\x20try:\n\
             \x20\x20\x20\x20    import xrun_hook  # noqa: F401  starts streamer in main.py\n\
             \x20\x20\x20\x20except Exception as _e:\n\
             \x20\x20\x20\x20    print('xrun_hook import failed:', _e, flush=True)\n\
             # Block subprocess re-imports of xrun_hook from creating a second\n\
             # streamer (which would push duplicate chunks to a separate MLflow\n\
             # run). User code can still call xrun_hook.metric/.epoch/.done — only\n\
             # the log streamer is suppressed downstream.\n\
             os.environ['XRUN_LOG_STREAM_DISABLE'] = '1'\n",
        ),
        None => String::new(),
    };

    format!(
        "import subprocess, sys, os\n\
         {env_prelude}\
         \n\
         os.makedirs({workdir_repr}, exist_ok=True)\n\
         os.chdir({workdir_repr})\n\
         \n\
         # Probe dataset mount path (old and new Kaggle API)\n\
         def _find_input_dir(datasets):\n\
         \x20\x20\x20\x20for ds in datasets:\n\
         \x20\x20\x20\x20    parts = ds.split('/')\n\
         \x20\x20\x20\x20    if len(parts) == 2:\n\
         \x20\x20\x20\x20        owner, name = parts\n\
         \x20\x20\x20\x20        for candidate in [\n\
         \x20\x20\x20\x20            f'/kaggle/input/datasets/{{owner}}/{{name}}',\n\
         \x20\x20\x20\x20            f'/kaggle/input/{{name}}',\n\
         \x20\x20\x20\x20        ]:\n\
         \x20\x20\x20\x20            if os.path.isdir(candidate):\n\
         \x20\x20\x20\x20                return candidate\n\
         \x20\x20\x20\x20return '/kaggle/input'\n\
         \n\
         _DATASETS = {datasets_repr}\n\
         os.environ['XRUN_INPUT_DIR'] = _find_input_dir(_DATASETS)\n\
         \n\
         {wheel_block}\
         _LOG = open('__xrun_stdout.log', 'w')\n\
         \n\
         def _run(script, label):\n\
         \x20\x20\x20\x20if not script.strip():\n\
         \x20\x20\x20\x20    return\n\
         \x20\x20\x20\x20header = '=== ' + label + ' ==='\n\
         \x20\x20\x20\x20print(header, flush=True)\n\
         \x20\x20\x20\x20_LOG.write(header + '\\n'); _LOG.flush()\n\
         \x20\x20\x20\x20proc = subprocess.Popen(\n\
         \x20\x20\x20\x20    ['bash', '-c', script],\n\
         \x20\x20\x20\x20    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=1)\n\
         \x20\x20\x20\x20for raw in proc.stdout:\n\
         \x20\x20\x20\x20    line = raw.decode('utf-8', errors='replace')\n\
         \x20\x20\x20\x20    print(line, end='', flush=True)\n\
         \x20\x20\x20\x20    _LOG.write(line); _LOG.flush()\n\
         \x20\x20\x20\x20proc.wait()\n\
         \x20\x20\x20\x20if proc.returncode != 0:\n\
         \x20\x20\x20\x20    msg = label + ' failed with exit code ' + str(proc.returncode)\n\
         \x20\x20\x20\x20    print(msg, flush=True)\n\
         \x20\x20\x20\x20    _LOG.write(msg + '\\n'); _LOG.flush()\n\
         \x20\x20\x20\x20    sys.exit(proc.returncode)\n\
         \n\
         _run('''{setup_escaped}''', 'setup')\n\
         _run('''{cmd_escaped}''', 'cmd')\n\
         \n\
         _LOG.close()\n",
        env_prelude = env_prelude,
        workdir_repr = workdir_repr,
        datasets_repr = datasets_repr,
        wheel_block = wheel_block,
        setup_escaped = setup_escaped,
        cmd_escaped = cmd_escaped
    )
}

/// Standard base64 (RFC 4648) encoder used for the embedded wheel. We avoid
/// pulling the `base64` crate in just for this — Python's `base64.b64decode`
/// accepts the same alphabet on the receive side.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPH: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let b = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPH[((b >> 18) & 0x3f) as usize] as char);
        out.push(ALPH[((b >> 12) & 0x3f) as usize] as char);
        out.push(ALPH[((b >> 6) & 0x3f) as usize] as char);
        out.push(ALPH[(b & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let b = (bytes[i] as u32) << 16;
        out.push(ALPH[((b >> 18) & 0x3f) as usize] as char);
        out.push(ALPH[((b >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPH[((b >> 18) & 0x3f) as usize] as char);
        out.push(ALPH[((b >> 12) & 0x3f) as usize] as char);
        out.push(ALPH[((b >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

/// Poll Kaggle kernel status until completion. Returns final RunId status.
/// Used by integration tests; in production the `Poller` calls `poll_completion`.
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
