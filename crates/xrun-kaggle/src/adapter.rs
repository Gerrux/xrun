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
    parse_chunk_seq, parse_chunk_seq_with, slice_from_offset, ARTIFACT_PREFIX, EVENTS_PREFIX,
    EVENTS_STEM, LOG_STREAM_EXPERIMENT, LOG_STREAM_FILE, METRICS_PREFIX, METRICS_STEM, TAG_RUN_ID,
    TELEMETRY_EXT,
};
use crate::notebook_inject;
use xrun_core::store::{NewEvent, NewMetric};

/// Wrapper script injected as `main.py` for script-mode kernels.
pub const XRUN_KAGGLE_ENTRY_PY: &str = include_str!("../tests/data/_xrun_kaggle_entry.py");

/// MLflow tracking-server config used for the live-log side channel. When
/// present, `provision()` injects MLFLOW_* env vars into the kernel — into
/// the generated `main.py` for script-mode and into a prepended prelude cell
/// for notebook-mode — so xrun_hook can stream chunks back to MLflow, where
/// `tail()` and `ingest_telemetry_chunks()` pull them.
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

    /// Resolve the Kaggle username for `{user}` placeholder expansion.
    ///
    /// Order:
    ///   1. `credentials.username` — legacy username+key auth carries it
    ///      directly, no extra round-trip.
    ///   2. `cli.authenticate()` — token-only auth has no username locally,
    ///      so we ask the kaggle Python module which knows how to derive
    ///      it from the access token.
    ///
    /// Returns `None` only if neither source produced a username — in that
    /// case `expand_kernel_slug` leaves `{user}` literal, and the
    /// downstream Kaggle push will fail with a clear "invalid slug" error
    /// instead of silently uploading under a nonsensical handle.
    fn resolve_user(&self) -> Option<String> {
        if let Some(u) = self.credentials.username.as_deref() {
            if !u.is_empty() {
                return Some(u.to_string());
            }
        }
        self.cli.authenticate().ok()
    }

    /// `store_path` holds the data directory (parent of `runs.db`) so the
    /// adapter can derive sibling paths like `runs/<id>/artifacts`. The
    /// SQLite store itself lives at `<data_dir>/runs.db` — every callsite
    /// that opens it must go through this helper rather than passing
    /// `store_path` directly.
    fn db_path(&self) -> Option<PathBuf> {
        self.store_path.as_ref().map(|p| p.join("runs.db"))
    }

    /// Reconstruct the highest-reached kernel state from the DB so a freshly
    /// spawned poll-daemon doesn't re-emit transitions already recorded by a
    /// previous daemon. Returns `None` when we cannot read the store or no
    /// transition events exist yet.
    fn recover_last_kernel_state(&self) -> Option<KernelState> {
        let db_path = self.db_path()?;
        let run_id = self.get_run_id()?;
        let store = Store::open(&db_path).ok()?;
        let events = store.list_events(&run_id).ok()?;
        let mut highest: Option<KernelState> = None;
        for ev in events {
            if ev.status != "start" {
                continue;
            }
            match ev.stage.as_str() {
                "queued" if highest.is_none() => {
                    highest = Some(KernelState::Queued);
                }
                "running" => highest = Some(KernelState::Running),
                _ => {}
            }
        }
        highest
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
            format!("os.environ['MLFLOW_TRACKING_URI'] = {}", py_str(&cfg.url)),
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

        // Slug must carry an owner before the slash. `{user}` is allowed as
        // a placeholder for the owner half — it's resolved from credentials
        // at provision time. Anything past the slash is the kernel name and
        // is otherwise validated by Kaggle's API.
        if !kaggle.kernel_slug.contains('/') {
            return Err(VendorError::Validation(
                "kaggle.kernel_slug must be in format <username>/<slug> \
                 (use `{user}/<slug>` to auto-fill the owner from credentials)"
                    .to_string(),
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

        // Show the slug the operator will see in `xrun ls`, not the raw
        // template. We don't have a `run_id` yet at dry-run time so `{run_id}`
        // expands to a fallback timestamp — that's fine, it's just for display.
        let preview_slug =
            expand_kernel_slug(&kaggle.kernel_slug, None, self.resolve_user().as_deref());

        Ok(DryRunPlan {
            gpu_query: format!(
                "kaggle:{}{}",
                preview_slug,
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

        // Expand placeholders in kernel_slug. Supported: `{run_id}` (unique
        // per launch — guarantees no collision on rerun), `{date}` (today
        // in YYYYMMDD, one collision per day worst case), `{user}` (Kaggle
        // account name, lets templates ship without the operator typing
        // their handle into YAML). Hardcoding a date suffix in YAML and
        // editing it before each rerun is the failure mode we're closing.
        let resolved_user = self.resolve_user();
        let expanded_slug = expand_kernel_slug(
            &kaggle.kernel_slug,
            self.get_run_id().as_ref().map(|r| r.to_string()).as_deref(),
            resolved_user.as_deref(),
        );
        if expanded_slug.contains("{user}") {
            return Err(VendorError::Validation(
                "kernel_slug uses {user} but no Kaggle username could be resolved \
                 from credentials. Set kaggle.username/.key (legacy) or run \
                 `kaggle config view` to confirm token auth is healthy."
                    .to_string(),
            ));
        }
        let kernel_slug = expanded_slug.as_str();

        let enable_gpu = kaggle.enable_gpu.unwrap_or(false);
        let enable_internet = kaggle.enable_internet.unwrap_or(false);
        let is_notebook = manifest.run.notebook.is_some();

        // Collect dataset slugs: legacy single `dataset` + new `datasets` list.
        let mut dataset_sources: Vec<String> = {
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

        // §8b: Pin each dataset to its current version number.
        //
        // `datasets status = ready` only confirms the storage layer has the
        // blob; the kernel-creation API resolves "latest" through a separate
        // cache that can lag minutes behind. Without an explicit version,
        // kernels sometimes mount the previous snapshot and crash on missing
        // files (Issue 1 in field-issues log). Slugs that already carry a
        // `/N` suffix are left alone — the user explicitly pinned them.
        if let Some(auth) = http::auth_from_credentials(&self.credentials) {
            if let Ok(client) = KaggleApiClient::new(auth) {
                for slug in dataset_sources.iter_mut() {
                    if has_version_suffix(slug) {
                        continue;
                    }
                    match client.dataset_current_version(slug) {
                        Ok(Some(n)) => {
                            tracing::info!("pinning dataset '{slug}' to version {n}");
                            *slug = format!("{slug}/{n}");
                        }
                        Ok(None) => {
                            tracing::debug!(
                                "could not resolve version for dataset '{slug}' — \
                                 leaving unpinned (kernel may pick stale snapshot)"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "dataset version lookup failed for '{slug}': {e} — \
                                 leaving unpinned"
                            );
                        }
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
            // Read the user's notebook and prepend a synthetic xrun-bootstrap
            // cell that installs xrun_hook + sets MLFLOW_* env vars before
            // any user cell runs. Without this, kernel-side `xrun_hook.metric`
            // calls silently fail (no MLFLOW_TRACKING_URI), and `xrun events`
            // sees only host-side queue/running events.
            let nb_bytes = std::fs::read(nb_path)
                .map_err(|e| VendorError::Other(format!("failed to read notebook: {e}")))?;
            let nb_str = String::from_utf8(nb_bytes)
                .map_err(|e| VendorError::Other(format!("notebook is not valid UTF-8: {e}")))?;
            let env_prelude = self.build_env_prelude();
            let wheel_b64 = if !embed::XRUN_HOOK_WHEEL.is_empty() {
                Some(base64_encode(embed::XRUN_HOOK_WHEEL))
            } else {
                None
            };
            let injected =
                notebook_inject::inject_bootstrap_cell(&nb_str, &env_prelude, wheel_b64.as_deref())
                    .map_err(|e| VendorError::Other(format!("notebook injection failed: {e}")))?;
            std::fs::write(staging.join(nb_name), injected.as_bytes())
                .map_err(|e| VendorError::Other(format!("failed to write notebook: {e}")))?;
            (
                nb_name.to_string(),
                KernelMetadata::new_notebook(
                    kernel_slug,
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
                    kernel_slug,
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

        // The xrun_hook wheel is base64-embedded into main.py (script-mode)
        // or into a synthetic prelude cell prepended to the user's .ipynb
        // (notebook-mode). Both paths give kernel-side code the same MLflow
        // env + xrun_hook install. When the wheel is missing at build time
        // (clean checkout, no Python toolchain) the kernel still runs but
        // xrun_hook is unavailable — warn so the empty-telemetry symptom
        // doesn't surprise the operator.
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

        let artifacts =
            match block_async(client.list_artifacts(&artifact_path, Some(ARTIFACT_PREFIX))) {
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
                let db_path = store_path.join("runs.db");
                match Store::open(&db_path) {
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
        match self.cli.authenticate() {
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
        // §6: hydrate last-state from DB on first poll after a daemon restart.
        // Without this, every restart re-emits `queued:start` / `running:start`
        // because the in-memory transition tracker was lost with the old
        // process. Field-issue: duplicate `running:start` events in `xrun show`.
        if last_guard.is_none() {
            *last_guard = self.recover_last_kernel_state();
        }
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

        // Pull any live event/metric chunks pushed by xrun_hook → MLflow
        // since the last tick. No-op when MLflow isn't configured. Done
        // before the terminal-state branch so the final partial chunk is
        // still ingested if `Complete` lands on this tick.
        let streamed_terminal = self.ingest_telemetry_chunks();

        match new_state {
            KernelState::Complete => {
                // Promote terminal status immediately. We deliberately do NOT
                // auto-pull `kaggle kernels output` here: that download can be
                // multi-GB and minutes long, and the only callers that need
                // this function to return fast (`xrun fix-status`, the TUI's
                // `S` action with a 60s subprocess timeout) would get killed
                // mid-pull and leave the run stuck in `running ⚠ stale`.
                // Telemetry already lives in the configured sink (MLflow live
                // chunks via `ingest_telemetry_chunks`, or wandb via the
                // hook → vendor API). Heavy artifacts are pulled on demand
                // through `xrun pull <id>`, which is the user's explicit
                // checkpoint-fetch path.
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
                // Streamed events from xrun_hook have already latched a
                // terminal status (e.g. `train:fail` on a CUDA error) while
                // Kaggle's coarse-grained kernel state is still Running.
                // Promote the run now and cancel the kernel so the user
                // doesn't keep paying for compute that's already toast.
                if let Some(terminal) = streamed_terminal {
                    tracing::info!(
                        "kaggle poll_completion {slug}: promoting run to {} from streamed events \
                         (kernel state still {:?}); cancelling kernel",
                        terminal.as_str(),
                        new_state,
                    );
                    if let Err(e) = self.destroy(h) {
                        tracing::warn!("kaggle poll_completion: cancel after early-fail: {e}");
                    }
                    return Some(PollCompletion {
                        terminal_status: Some(terminal),
                        events,
                    });
                }
                // Return Some-empty rather than None even when there's no
                // transition. `xrun resume` distinguishes "kernel still
                // alive, respawn" from "vendor probe failed" by whether
                // `poll_completion` returns Some — falling through to
                // `vendor_instances` on None breaks resume because kaggle's
                // `kernels list` CLI doesn't emit JSON. The loop_runner is
                // unaffected: empty events + no terminal is a no-op there.
                Some(PollCompletion {
                    terminal_status: None,
                    events,
                })
            }
        }
    }
}

impl KaggleAdapter {
    /// Pull new events/metrics chunks from MLflow and ingest them into the
    /// store. This is what closes the "no live telemetry on Kaggle" gap
    /// (Issue 8): xrun_hook's streamer pushes events.jsonl/metrics.jsonl
    /// chunks to MLflow as artifacts, and we tail them on every poll tick.
    ///
    /// Returns `Some(RunStatus)` when an ingested event signals terminal
    /// state (`status=fail` → `Failed`, `stage=done` + `status=ok` → `Done`),
    /// so `poll_completion` can promote the run before Kaggle's coarse-grained
    /// `KernelState` flips to `Error`/`Complete`. Otherwise `None`.
    ///
    /// Best-effort — every error path is swallowed and tracing'd so a
    /// transient MLflow blip doesn't break the kernel-state poller.
    fn ingest_telemetry_chunks(&self) -> Option<RunStatus> {
        let cfg = self.mlflow.as_ref()?;
        let xrun_run_id = self.get_run_id()?;
        let db_path = self.db_path()?;

        let client = MlflowClient::new(cfg.url.clone(), cfg.auth.clone());
        let resolved = match self.resolve_mlflow_run(&client, &xrun_run_id.to_string()) {
            Ok(Some(pair)) => pair,
            Ok(None) => return None,
            Err(e) => {
                tracing::debug!("kaggle telemetry: mlflow run lookup failed: {e}");
                return None;
            }
        };
        let (_mlflow_run_id, artifact_path) = resolved;

        let mut store = match Store::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("kaggle telemetry: store open failed: {e}");
                return None;
            }
        };

        let mut terminal: Option<RunStatus> = None;

        // Events
        let events_offset = store
            .get_poll_offset(&xrun_run_id, "telemetry_events")
            .unwrap_or(0);
        if let Some(new_bytes) = self.fetch_chunks_past_offset(
            &client,
            &artifact_path,
            EVENTS_PREFIX,
            EVENTS_STEM,
            events_offset,
        ) {
            let parsed = parse_event_lines(&new_bytes);
            let count = parsed.len();
            for ev in parsed {
                // Latch the first terminal signal we see in this batch so
                // poll_completion can promote the run before Kaggle's
                // KernelState catches up.
                if terminal.is_none() {
                    terminal = detect_terminal_event(&ev);
                }
                if let Err(e) = store.append_event(&xrun_run_id, ev) {
                    tracing::warn!("kaggle telemetry: append_event failed: {e}");
                }
            }
            let new_offset = events_offset + new_bytes.len() as u64;
            let _ = store.update_poll_offset(&xrun_run_id, "telemetry_events", new_offset);
            if count > 0 {
                tracing::debug!("kaggle telemetry: ingested {count} live events");
            }
        }

        // Metrics
        let metrics_offset = store
            .get_poll_offset(&xrun_run_id, "telemetry_metrics")
            .unwrap_or(0);
        if let Some(new_bytes) = self.fetch_chunks_past_offset(
            &client,
            &artifact_path,
            METRICS_PREFIX,
            METRICS_STEM,
            metrics_offset,
        ) {
            let parsed = parse_metric_lines(&new_bytes);
            let count = parsed.len();
            for m in parsed {
                if let Err(e) = store.append_metric(&xrun_run_id, m) {
                    tracing::warn!("kaggle telemetry: append_metric failed: {e}");
                }
            }
            let new_offset = metrics_offset + new_bytes.len() as u64;
            let _ = store.update_poll_offset(&xrun_run_id, "telemetry_metrics", new_offset);
            if count > 0 {
                tracing::debug!("kaggle telemetry: ingested {count} live metrics");
            }
        }

        terminal
    }

    /// List MLflow artifacts under `prefix`, sort by chunk seq, and return
    /// the bytes appended past `offset`. Returns None on any HTTP/parse error
    /// so callers leave the offset untouched and retry next tick.
    fn fetch_chunks_past_offset(
        &self,
        client: &MlflowClient,
        artifact_path: &str,
        prefix: &str,
        stem: &str,
        offset: u64,
    ) -> Option<Vec<u8>> {
        let artifacts = match block_async(client.list_artifacts(artifact_path, Some(prefix))) {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!("kaggle telemetry: artifacts/list({prefix}) failed: {e}");
                return None;
            }
        };
        let mut chunks: Vec<(u32, String, u64)> = artifacts
            .into_iter()
            .filter_map(|(p, sz)| {
                parse_chunk_seq_with(stem, TELEMETRY_EXT, &p).map(|seq| (seq, p, sz))
            })
            .collect();
        if chunks.is_empty() {
            return None;
        }
        chunks.sort_by_key(|(seq, _, _)| *seq);

        let result: Result<Vec<u8>, VendorError> =
            slice_from_offset::<VendorError>(&chunks, offset, |idx| {
                let path = &chunks[idx].1;
                block_async(client.download_artifact(artifact_path, path))
                    .map_err(|e| VendorError::Other(format!("download_artifact: {e}")))
            });
        match result {
            Ok(bytes) if bytes.is_empty() => None,
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::debug!("kaggle telemetry: chunk fetch failed: {e}");
                None
            }
        }
    }
}

/// Parse `[{ts, stage, status, msg?, extra?}, …]` lines from a JSONL byte
/// buffer streamed off MLflow into [`NewEvent`] records. Bad lines are
/// skipped with a debug log so the rest of the chunk still ingests.
/// Mirror of the loop_runner's terminal-status rule (`status=fail` →
/// Failed, `stage=done` + `status=ok` → Done) so streamed-event ingestion
/// can latch terminal state symmetrically with the tail-based vendors.
fn detect_terminal_event(ev: &NewEvent) -> Option<RunStatus> {
    if ev.status == "fail" {
        Some(RunStatus::Failed)
    } else if ev.stage == "done" && ev.status == "ok" {
        Some(RunStatus::Done)
    } else {
        None
    }
}

fn parse_event_lines(bytes: &[u8]) -> Vec<NewEvent> {
    use chrono::DateTime;
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("kaggle telemetry: skip bad event line: {e}");
                continue;
            }
        };
        let ts_str = match v.get("ts").and_then(|x| x.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let ts = match DateTime::parse_from_rfc3339(ts_str) {
            Ok(t) => t.with_timezone(&Utc),
            Err(_) => continue,
        };
        let stage = v
            .get("stage")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_string();
        let status = v
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("ok")
            .to_string();
        let msg = v.get("msg").and_then(|x| x.as_str()).map(str::to_string);
        let payload_json = v.get("extra").and_then(|x| {
            if x.is_object() && x.as_object().is_some_and(|o| !o.is_empty()) {
                Some(x.to_string())
            } else {
                None
            }
        });
        out.push(NewEvent {
            ts,
            stage,
            status,
            msg,
            payload_json,
        });
    }
    out
}

fn parse_metric_lines(bytes: &[u8]) -> Vec<NewMetric> {
    use chrono::DateTime;
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ts = v
            .get("ts")
            .and_then(|x| x.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        let key = v
            .get("key")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let value = v.get("value").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let step = v.get("step").and_then(|x| x.as_i64()).unwrap_or(0);
        if key.is_empty() || ts.is_none() {
            continue;
        }
        out.push(NewMetric {
            ts: ts.unwrap(),
            step,
            key,
            value,
        });
    }
    out
}

/// True when the slug already carries an explicit `/<version>` suffix,
/// e.g. `alice/dataset/3`. Two slashes mean the user pinned a version.
fn has_version_suffix(slug: &str) -> bool {
    slug.matches('/').count() >= 2
        && slug
            .rsplit('/')
            .next()
            .is_some_and(|tail| tail.chars().all(|c| c.is_ascii_digit()))
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
         \n\
         # numpy↔torch ABI probe. If torch was pinned against numpy 1.x but the\n\
         # environment has numpy 2.x (or vice versa), DataLoader workers blow up\n\
         # 4 minutes into training with `RuntimeError: Numpy is not available`.\n\
         # Surface that immediately with a clear message instead.\n\
         def _probe_torch_numpy():\n\
         \x20\x20\x20\x20probe = (\n\
         \x20\x20\x20\x20    'import sys\\n'\n\
         \x20\x20\x20\x20    'try:\\n'\n\
         \x20\x20\x20\x20    '    import torch\\n'\n\
         \x20\x20\x20\x20    'except Exception:\\n'\n\
         \x20\x20\x20\x20    '    sys.exit(0)\\n'\n\
         \x20\x20\x20\x20    'import numpy as _np\\n'\n\
         \x20\x20\x20\x20    'torch.from_numpy(_np.zeros(1))\\n'\n\
         \x20\x20\x20\x20)\n\
         \x20\x20\x20\x20r = subprocess.run([sys.executable, '-c', probe], capture_output=True, text=True)\n\
         \x20\x20\x20\x20if r.returncode != 0:\n\
         \x20\x20\x20\x20    msg = ('xrun: torch<->numpy ABI probe FAILED — pin numpy<2 if torch was '\n\
         \x20\x20\x20\x20           'built against numpy 1.x (or upgrade torch). DataLoader workers '\n\
         \x20\x20\x20\x20           'would crash several minutes in.\\n' + r.stderr)\n\
         \x20\x20\x20\x20    print(msg, flush=True)\n\
         \x20\x20\x20\x20    _LOG.write(msg + '\\n'); _LOG.flush()\n\
         \x20\x20\x20\x20    sys.exit(r.returncode)\n\
         _probe_torch_numpy()\n\
         \n\
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

/// Expand `{run_id}`, `{date}`, and `{user}` placeholders in a Kaggle kernel
/// slug.
/// - `{run_id}` is replaced by the lowercase ULID (Kaggle slugs are
///   case-insensitive but lowercase reads better). When `run_id` is `None`
///   the placeholder is replaced with a millisecond timestamp — collisions
///   across millisecond boundaries are vanishingly rare in practice and a
///   stray fallback beats panicking on a missing run_id.
/// - `{date}` is `YYYYMMDD` UTC.
/// - `{user}` is the Kaggle account name. Lets templates ship as
///   `kernel_slug: {user}/xrun-foo` and "just work" without the operator
///   editing in their handle. When `user` is `None` the placeholder is left
///   intact so callers can detect an unresolvable slug and surface a clear
///   error instead of pushing a kernel under a literal `{user}` namespace.
pub fn expand_kernel_slug(slug: &str, run_id: Option<&str>, user: Option<&str>) -> String {
    if !slug.contains('{') {
        return slug.to_string();
    }
    let date = chrono::Utc::now().format("%Y%m%d").to_string();
    let rid_owned: String;
    let rid: &str = match run_id {
        Some(s) => s,
        None => {
            rid_owned = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
            rid_owned.as_str()
        }
    };
    let mut out = slug
        .replace("{run_id}", &rid.to_lowercase())
        .replace("{date}", &date);
    if let Some(u) = user {
        out = out.replace("{user}", u);
    }
    out
}

#[cfg(test)]
mod terminal_detection_tests {
    use super::{detect_terminal_event, NewEvent, RunStatus};
    use chrono::Utc;

    fn ev(stage: &str, status: &str) -> NewEvent {
        NewEvent {
            ts: Utc::now(),
            stage: stage.into(),
            status: status.into(),
            msg: None,
            payload_json: None,
        }
    }

    #[test]
    fn fail_status_promotes_to_failed() {
        assert_eq!(
            detect_terminal_event(&ev("train", "fail")),
            Some(RunStatus::Failed)
        );
        assert_eq!(
            detect_terminal_event(&ev("error", "fail")),
            Some(RunStatus::Failed)
        );
    }

    #[test]
    fn done_ok_promotes_to_done() {
        assert_eq!(
            detect_terminal_event(&ev("done", "ok")),
            Some(RunStatus::Done)
        );
    }

    #[test]
    fn ok_on_other_stages_is_not_terminal() {
        assert_eq!(detect_terminal_event(&ev("data_load", "ok")), None);
        assert_eq!(detect_terminal_event(&ev("train", "ok")), None);
    }

    #[test]
    fn start_and_progress_are_not_terminal() {
        assert_eq!(detect_terminal_event(&ev("train", "start")), None);
        assert_eq!(detect_terminal_event(&ev("epoch", "progress")), None);
    }
}

#[cfg(test)]
mod slug_tests {
    use super::expand_kernel_slug;

    #[test]
    fn passthrough_when_no_placeholder() {
        assert_eq!(
            expand_kernel_slug("user/foo-bar", Some("01H..."), Some("alice")),
            "user/foo-bar"
        );
    }

    #[test]
    fn substitutes_run_id_lowercase() {
        let s = expand_kernel_slug("user/foo-{run_id}", Some("01HABC"), None);
        assert_eq!(s, "user/foo-01habc");
    }

    #[test]
    fn substitutes_date() {
        let s = expand_kernel_slug("user/foo-{date}", Some("01H"), None);
        assert!(s.starts_with("user/foo-"));
        assert_eq!(s.len(), "user/foo-".len() + 8);
    }

    #[test]
    fn falls_back_when_no_run_id() {
        let s = expand_kernel_slug("user/foo-{run_id}", None, None);
        assert!(s.starts_with("user/foo-"));
        assert!(!s.contains('{'));
    }

    #[test]
    fn substitutes_user() {
        let s = expand_kernel_slug("{user}/foo-bar", None, Some("kartaviychert"));
        assert_eq!(s, "kartaviychert/foo-bar");
    }

    #[test]
    fn user_combined_with_run_id() {
        let s = expand_kernel_slug("{user}/exp-{run_id}", Some("01HABC"), Some("alice"));
        assert_eq!(s, "alice/exp-01habc");
    }

    #[test]
    fn user_left_intact_when_unresolved() {
        // Caller (provision) detects {user} surviving the expansion and
        // surfaces a "no Kaggle username could be resolved" error rather
        // than uploading under a literal {user} namespace.
        let s = expand_kernel_slug("{user}/foo", None, None);
        assert_eq!(s, "{user}/foo");
    }
}

/// Standard base64 (RFC 4648) encoder used for the embedded wheel. We avoid
/// pulling the `base64` crate in just for this — Python's `base64.b64decode`
/// accepts the same alphabet on the receive side.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
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
