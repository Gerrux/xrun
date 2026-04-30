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
    /// Tracks the last observed kernel state to emit transition events exactly once.
    last_kernel_state: Mutex<Option<KernelState>>,
}

impl KaggleAdapter {
    pub fn new() -> Self {
        Self {
            cli: KaggleCli::new(),
            run_id: std::sync::RwLock::new(None),
            store_path: None,
            last_kernel_state: Mutex::new(None),
        }
    }

    pub fn with_process(process: Box<dyn KaggleProcess>) -> Self {
        Self {
            cli: KaggleCli::with_process(process),
            run_id: std::sync::RwLock::new(None),
            store_path: None,
            last_kernel_state: Mutex::new(None),
        }
    }

    pub fn with_store_path(mut self, path: PathBuf) -> Self {
        self.store_path = Some(path);
        self
    }

    /// Inject Kaggle credentials so the kaggle CLI subprocess can authenticate.
    /// Sets `KAGGLE_USERNAME`/`KAGGLE_KEY` (legacy) or `KAGGLE_API_TOKEN` (new Bearer).
    pub fn with_credentials(mut self, creds: KaggleCredentials) -> Self {
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(token) = creds.token {
            env.push(("KAGGLE_API_TOKEN".into(), token));
        } else if let (Some(user), Some(key)) = (creds.username, creds.key) {
            env.push(("KAGGLE_USERNAME".into(), user));
            env.push(("KAGGLE_KEY".into(), key));
        }
        if !env.is_empty() {
            self.cli = self.cli.with_env(env);
        }
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
            let main_py = build_script_main(&setup_block, &cmd_block, workdir, &dataset_sources);
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

    fn tail(&self, _h: &InstanceHandle, _file: &str, _offset: u64) -> Result<Vec<u8>, VendorError> {
        // Kaggle doesn't support live tail — completion is detected via poll_completion()
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
        // §4: Attempt to cancel the running kernel via `kaggle kernels cancel`.
        let slug = h.id.strip_prefix("kaggle:").unwrap_or(&h.id);
        match self.cli.cancel(slug) {
            Ok(()) => {
                tracing::info!("kaggle kernel '{slug}' cancel request sent");
            }
            Err(e) => {
                tracing::warn!(
                    "could not cancel kaggle kernel '{slug}': {e}\n\
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
fn build_script_main(setup: &str, cmd: &str, workdir: &str, datasets: &[String]) -> String {
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

    format!(
        "import subprocess, sys, os\n\
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
        workdir_repr = workdir_repr,
        datasets_repr = datasets_repr,
        setup_escaped = setup_escaped,
        cmd_escaped = cmd_escaped
    )
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
