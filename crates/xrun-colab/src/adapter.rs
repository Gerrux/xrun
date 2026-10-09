#![deny(unsafe_code)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use sha2::{Digest, Sha256};
use xrun_core::{
    error::VendorError,
    manifest::{validate as core_validate, DataSource, Manifest, RunSpec, Vendor},
    store::{NewArtifact, NewEvent, RunId, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter, VendorRemoteInstance, VendorStatus},
};
use xrun_ssh::{build_cmd_line, classify_kind, effective_run_dir, pull_pattern, training_dir};

use crate::bridge::{ColabBridge, PyColabBridge};
use crate::cmd::{self, ShOut};
use crate::error::ColabError;

/// Remote root when `colab.workdir` is not set.
pub const DEFAULT_WORKDIR: &str = "/content/xrun";
/// Accelerator when `colab.gpu` is not set.
pub const DEFAULT_GPU: &str = "T4";

/// Default budget of a `sh` call.
const SH_TIMEOUT: Duration = Duration::from_secs(90);
/// `run.setup` may install packages for a long time.
const SETUP_TIMEOUT: Duration = Duration::from_secs(3600);
/// Tail / probe / glob calls.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Google Colab adapter: a runtime session is the box, driven through the
/// Python `colab_cli` library (see `bridge.py`). Shell work goes through the
/// session kernel; the training process is detached from the kernel and its
/// files live under `<workdir>/<run_id>/` like on the ssh vendor.
pub struct ColabAdapter {
    store: RefCell<Option<Store>>,
    run_id: RefCell<Option<RunId>>,
    bridge: Box<dyn ColabBridge>,
    /// Remote root (absolute); the run dir is `<root>/<run_id>`.
    workdir_root: RefCell<String>,
}

impl ColabAdapter {
    /// Production adapter: the Python child is spawned lazily on first use.
    pub fn new(store: Store) -> Self {
        Self::with_bridge(store, Box::new(PyColabBridge::new()))
    }

    pub fn with_bridge(store: Store, bridge: Box<dyn ColabBridge>) -> Self {
        Self {
            store: RefCell::new(Some(store)),
            run_id: RefCell::new(None),
            bridge,
            workdir_root: RefCell::new(DEFAULT_WORKDIR.to_string()),
        }
    }

    /// Override the remote root (poll-daemon / pull paths that re-resolve a
    /// run dir without a manifest in hand). `provision` sets it from
    /// `colab.workdir` itself.
    pub fn with_workdir_root(self, root: impl Into<String>) -> Self {
        *self.workdir_root.borrow_mut() = root.into();
        self
    }

    /// Session name for a run: `xrun-<run_id>` lowercased.
    pub fn session_name(run_id: &RunId) -> String {
        format!("xrun-{}", run_id.to_string().to_lowercase())
    }

    /// Instance / handle id for a run.
    pub fn instance_id(run_id: &RunId) -> String {
        format!("colab-{}", Self::session_name(run_id))
    }

    fn run_id(&self) -> Result<RunId, VendorError> {
        self.run_id
            .borrow()
            .clone()
            .ok_or_else(|| VendorError::Other("ColabAdapter: run_id not set".into()))
    }

    fn run_dir_for(&self, h: &InstanceHandle, run_id: &RunId) -> String {
        effective_run_dir(
            h.run_dir.as_deref(),
            &self.workdir_root.borrow(),
            &run_id.to_string(),
        )
    }

    /// Session of a handle: the id minus the `colab-` prefix, else derived
    /// from the run.
    fn session_of(&self, h: &InstanceHandle) -> Result<String, VendorError> {
        match h.id.strip_prefix("colab-") {
            Some(s) if !s.is_empty() => Ok(s.to_string()),
            _ => Ok(Self::session_name(&self.run_id()?)),
        }
    }

    fn append_event(&self, stage: &str, status: &str, msg: Option<String>) {
        let Ok(run_id) = self.run_id() else {
            return;
        };
        let mut slot = self.store.borrow_mut();
        let Some(store) = slot.as_mut() else {
            return;
        };
        let _ = store.append_event(
            &run_id,
            NewEvent {
                ts: Utc::now(),
                stage: stage.to_string(),
                status: status.to_string(),
                msg,
                payload_json: None,
            },
        );
    }

    // ---- kernel helpers -------------------------------------------------

    /// Run a snippet in the session kernel; a raised exception is an error.
    fn kernel(&self, session: &str, code: &str, timeout: Duration) -> Result<String, ColabError> {
        let out = self.bridge.exec(session, code, timeout)?;
        Self::kernel_result(out)
    }

    /// [`Self::kernel`] for non-idempotent snippets: never replayed.
    fn kernel_once(
        &self,
        session: &str,
        code: &str,
        timeout: Duration,
    ) -> Result<String, ColabError> {
        let out = self.bridge.exec_once(session, code, timeout)?;
        Self::kernel_result(out)
    }

    fn kernel_result(out: crate::bridge::ExecOutput) -> Result<String, ColabError> {
        if let Some(e) = out.error {
            let mut detail = e;
            if !out.stderr.trim().is_empty() {
                detail = format!("{detail} ({})", out.stderr.trim());
            }
            return Err(ColabError::BadOutput {
                what: "kernel",
                detail,
            });
        }
        Ok(out.stdout)
    }

    fn sh(&self, session: &str, command: &str, timeout: Duration) -> Result<ShOut, ColabError> {
        cmd::parse_sh(&self.kernel(session, &cmd::sh_snippet(command), timeout)?)
    }

    fn sh_ok(&self, session: &str, command: &str, timeout: Duration) -> Result<ShOut, ColabError> {
        self.sh(session, command, timeout)?.ok()
    }

    /// [`Self::sh_ok`] without bridge replay (setup, kill).
    fn sh_ok_once(
        &self,
        session: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<ShOut, ColabError> {
        cmd::parse_sh(&self.kernel_once(session, &cmd::sh_snippet(command), timeout)?)?.ok()
    }

    // ---- upload helpers -------------------------------------------------

    /// `(local file, absolute remote path)` pairs for one data source.
    fn plan_upload(src: &DataSource) -> Result<Vec<(PathBuf, String)>, VendorError> {
        if !src.dst.starts_with('/') {
            return Err(VendorError::Validation(format!(
                "vendor=colab: data dst must be an absolute path, got {:?}",
                src.dst
            )));
        }
        let local = PathBuf::from(&src.src);
        let meta = std::fs::metadata(&local)
            .map_err(|e| VendorError::Other(format!("data src {}: {e}", src.src)))?;
        if meta.is_dir() {
            let root = src.dst.trim_end_matches('/');
            let mut files = Vec::new();
            collect_files(&local, "", &mut files)?;
            files.sort();
            Ok(files
                .into_iter()
                .map(|(p, rel)| (p, format!("{root}/{rel}")))
                .collect())
        } else {
            let remote = if src.dst.ends_with('/') {
                let name = local.file_name().and_then(|n| n.to_str()).unwrap_or("file");
                format!("{}{name}", src.dst)
            } else {
                src.dst.clone()
            };
            Ok(vec![(local, remote)])
        }
    }
}

/// All regular files under `dir`, with `/`-separated paths relative to it.
fn collect_files(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(PathBuf, String)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_files(&entry.path(), &rel, out)?;
        } else if ty.is_file() {
            out.push((entry.path(), rel));
        }
    }
    Ok(())
}

fn parent_dir(remote: &str) -> Option<&str> {
    match remote.rsplit_once('/') {
        Some(("", _)) | None => None,
        Some((p, _)) => Some(p),
    }
}

/// Pull source: `~` stays (the glob snippet expands it on the remote),
/// everything else is anchored like the ssh vendor.
fn colab_pull_pattern(run_dir: &str, remote: &str) -> String {
    if remote == "~" || remote.starts_with("~/") {
        remote.to_string()
    } else {
        pull_pattern(run_dir, remote)
    }
}

fn sha256_of_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn gpu_of(manifest: &Manifest) -> String {
    manifest
        .colab
        .as_ref()
        .and_then(|c| c.gpu.clone())
        .filter(|g| !g.trim().is_empty())
        .map(|g| {
            if g.eq_ignore_ascii_case("cpu") {
                "cpu".to_string()
            } else {
                g.to_uppercase()
            }
        })
        .unwrap_or_else(|| DEFAULT_GPU.to_string())
}

fn workdir_of(manifest: &Manifest) -> String {
    manifest
        .colab
        .as_ref()
        .and_then(|c| c.workdir.clone())
        .filter(|w| !w.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_WORKDIR.to_string())
}

impl VendorAdapter for ColabAdapter {
    fn name(&self) -> &'static str {
        "colab"
    }

    fn set_run_id(&self, run_id: &RunId) {
        *self.run_id.borrow_mut() = Some(run_id.clone());
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        core_validate(manifest)?;
        if !matches!(manifest.vendor, Vendor::Colab) {
            return Err(VendorError::Validation(format!(
                "ColabAdapter requires vendor=colab, got {:?}",
                manifest.vendor
            )));
        }
        if manifest.run.cmd.is_none() {
            return Err(VendorError::Validation(
                "vendor=colab requires run.cmd (notebooks not supported)".into(),
            ));
        }
        for d in manifest.data.iter().flatten() {
            if !d.dst.starts_with('/') {
                return Err(VendorError::Validation(format!(
                    "vendor=colab: data dst must be an absolute path (e.g. /content/data/x), got {:?}",
                    d.dst
                )));
            }
        }
        Ok(())
    }

    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError> {
        self.validate(manifest)?;
        let mut data_items: Vec<(PathBuf, String)> = Vec::new();
        let mut data_total_bytes: u64 = 0;
        for source in manifest.data.iter().flatten() {
            let src = PathBuf::from(&source.src);
            data_total_bytes += std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            data_items.push((src, source.dst.clone()));
        }
        let cmd_base = manifest.run.cmd.as_deref().unwrap_or("");
        Ok(DryRunPlan {
            gpu_query: gpu_of(manifest),
            estimated_price_max: 0.0,
            data_total_bytes,
            data_items,
            cmd_line: build_cmd_line(cmd_base, &manifest.run),
        })
    }

    fn vendor_status(&self) -> Result<VendorStatus, VendorError> {
        let now = Utc::now();
        let status =
            |connected: bool, account: Option<String>, error: Option<String>| VendorStatus {
                connected,
                balance: None,
                currency: None,
                account,
                last_checked: now,
                error,
            };
        Ok(match self.bridge.whoami() {
            Ok(w) if w.logged_in => status(
                true,
                w.usage
                    .map(|u| u.lines().map(str::trim).collect::<Vec<_>>().join(" · ")),
                None,
            ),
            Ok(_) => status(
                false,
                None,
                Some("not logged in: run `xrun config login colab`".to_string()),
            ),
            Err(e) => status(false, None, Some(e.to_string())),
        })
    }

    fn vendor_instances(&self) -> Result<Vec<VendorRemoteInstance>, VendorError> {
        let rows: Vec<_> = {
            let slot = self.store.borrow();
            let Some(store) = slot.as_ref() else {
                return Ok(Vec::new());
            };
            store
                .list_active_instances()
                .map_err(|e| VendorError::Other(format!("list active: {e}")))?
                .into_iter()
                .filter(|i| i.vendor == "colab")
                .collect()
        };
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        // One listing for all rows; a failed listing marks them unknown.
        let listed = self.bridge.sessions().ok();
        Ok(rows
            .into_iter()
            .map(|inst| {
                let name = inst.id.strip_prefix("colab-").unwrap_or(&inst.id);
                let session = listed
                    .as_ref()
                    .and_then(|l| l.iter().find(|s| s.name == name));
                let status = match (&listed, session) {
                    (None, _) => "unknown",
                    (Some(_), Some(_)) => "running",
                    (Some(_), None) => "gone",
                };
                VendorRemoteInstance {
                    id: inst.id.clone(),
                    gpu: inst.gpu_type.clone(),
                    num_gpus: None,
                    dph_total: Some(0.0),
                    status: Some(status.to_string()),
                    uptime_secs: inst
                        .created_at
                        .map(|t| (Utc::now() - t).num_seconds().max(0) as u64),
                    ssh: session.map(|s| s.endpoint.clone()),
                    region: None,
                }
            })
            .collect())
    }

    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        self.validate(manifest)?;
        let run_id = self.run_id()?;

        let root = workdir_of(manifest);
        *self.workdir_root.borrow_mut() = root.clone();
        let gpu = gpu_of(manifest);
        let high_mem = manifest
            .colab
            .as_ref()
            .and_then(|c| c.high_mem)
            .unwrap_or(false);
        let session = Self::session_name(&run_id);
        let run_dir = xrun_ssh::remote_run_dir(&root, &run_id.to_string());

        self.append_event(
            "provision",
            "start",
            Some(format!("colab session={session} gpu={gpu}")),
        );
        let info = self
            .bridge
            .session_new(&session, &gpu, high_mem)
            .map_err(|e| {
                self.append_event("provision", "fail", Some(e.to_string()));
                VendorError::from(e)
            })?;

        // From here on a session exists: release it if provisioning fails.
        let release = |why: String| -> VendorError {
            let _ = self.bridge.session_stop(&session);
            self.append_event("provision", "fail", Some(why.clone()));
            VendorError::Other(why)
        };
        self.sh_ok(
            &session,
            &cmd::mkdir_script(&[run_dir.as_str()]),
            SH_TIMEOUT,
        )
        .map_err(|e| release(format!("mkdir {run_dir}: {e}")))?;

        let id = Self::instance_id(&run_id);
        // The store borrow must end before `release` (which logs an event).
        let inserted = match self.store.borrow_mut().as_mut() {
            Some(store) => {
                store.insert_instance(&id, "colab", Some(&run_id), Some(&gpu), None, Utc::now())
            }
            None => Ok(()),
        };
        inserted.map_err(|e| release(format!("insert instance: {e}")))?;
        self.append_event(
            "provision",
            "ok",
            Some(format!(
                "session={session} endpoint={} accelerator={} workdir={run_dir}",
                info.endpoint, info.accelerator
            )),
        );

        Ok(InstanceHandle {
            id,
            vendor: "colab".to_string(),
            ssh_host: Some(info.endpoint),
            ssh_port: None,
            ssh_user: "root".to_string(),
            run_dir: Some(run_dir),
        })
    }

    fn upload(&self, h: &InstanceHandle, sources: &[DataSource]) -> Result<(), VendorError> {
        if sources.is_empty() {
            self.append_event("upload", "ok", Some("no data sources".into()));
            return Ok(());
        }
        let session = self.session_of(h)?;
        self.append_event(
            "upload",
            "start",
            Some(format!("{} sources", sources.len())),
        );
        let fail = |msg: String| {
            self.append_event("upload", "fail", Some(msg.clone()));
            VendorError::Other(msg)
        };
        let mut total = 0usize;
        for src in sources {
            let plan = Self::plan_upload(src).inspect_err(|e| {
                self.append_event("upload", "fail", Some(e.to_string()));
            })?;
            let parents: BTreeSet<&str> = plan.iter().filter_map(|(_, r)| parent_dir(r)).collect();
            if !parents.is_empty() {
                let dirs: Vec<&str> = parents.into_iter().collect();
                self.sh_ok(&session, &cmd::mkdir_script(&dirs), SH_TIMEOUT)
                    .map_err(|e| fail(format!("mkdir for {}: {e}", src.dst)))?;
            }
            for (local, remote) in &plan {
                self.bridge
                    .upload(&session, local, remote)
                    .map_err(|e| fail(format!("upload {} -> {remote}: {e}", local.display())))?;
                total += 1;
            }
        }
        self.append_event("upload", "ok", Some(format!("{total} files")));
        Ok(())
    }

    fn execute(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let session = self.session_of(h)?;
        let run_dir = self.run_dir_for(h, &run_id);
        let workdir = training_dir(run_spec, &run_dir);
        let cmd_base = run_spec
            .cmd
            .as_deref()
            .ok_or_else(|| VendorError::Validation("run.cmd required for colab".into()))?;

        self.sh_ok(
            &session,
            &cmd::mkdir_script(&[workdir.as_str()]),
            SH_TIMEOUT,
        )?;

        if let Some(setup) = run_spec.setup.as_deref().filter(|s| !s.trim().is_empty()) {
            self.append_event("env_ready", "start", None);
            let script = format!(
                "cd {wd} && ({setup})",
                wd = xrun_ssh::cmd::shell_quote(&workdir)
            );
            self.sh_ok_once(&session, &script, SETUP_TIMEOUT)
                .map_err(|e| {
                    self.append_event("env_ready", "fail", Some(e.to_string()));
                    VendorError::Other(format!("setup failed: {e}"))
                })?;
            self.append_event("env_ready", "ok", None);
        }

        let user_cmd = build_cmd_line(cmd_base, run_spec);
        let script = cmd::launch_script(&run_dir, &workdir, &run_id.to_string(), &user_cmd);
        let spawned = cmd::parse_spawn(&self.kernel_once(
            &session,
            &cmd::spawn_snippet(&script),
            Duration::from_secs(120),
        )?)?;

        // The pid that matters is the one the script recorded.
        let pid_file = format!("{run_dir}/run.pid");
        let pid = self
            .sh(
                &session,
                &format!("cat {}", xrun_ssh::cmd::shell_quote(&pid_file)),
                SH_TIMEOUT,
            )
            .ok()
            .filter(|o| o.code == 0)
            .map(|o| o.out.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| spawned.pid.map(|p| p.to_string()))
            .unwrap_or_default();
        self.append_event(
            "train_start",
            "ok",
            Some(format!("pid={pid} session={session}")),
        );
        Ok(())
    }

    fn tail(&self, h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError> {
        let session = self.session_of(h)?;
        let out = self.kernel(&session, &cmd::tail_snippet(file, offset), PROBE_TIMEOUT)?;
        let chunk = cmd::parse_tail(&out)?;
        if chunk.size < offset {
            return Err(VendorError::Truncated);
        }
        Ok(chunk.data)
    }

    fn pull(&self, h: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let session = self.session_of(h)?;
        let run_dir = self.run_dir_for(h, &run_id);
        let pattern = colab_pull_pattern(&run_dir, remote);

        let files = cmd::parse_glob(&self.kernel(
            &session,
            &cmd::glob_snippet(&pattern),
            PROBE_TIMEOUT,
        )?)?;
        if files.is_empty() {
            return Err(VendorError::Other(format!(
                "pull {pattern}: no files match"
            )));
        }
        std::fs::create_dir_all(into)
            .map_err(|e| VendorError::Other(format!("create local pull dir: {e}")))?;

        for remote_file in &files {
            let base = remote_file.rsplit('/').next().unwrap_or(remote_file);
            let local = into.join(base);
            self.bridge
                .download(&session, remote_file, &local)
                .map_err(|e| VendorError::Other(format!("download {remote_file}: {e}")))?;
            if let Some(store) = self.store.borrow_mut().as_mut() {
                let size = std::fs::metadata(&local).map(|m| m.len() as i64).ok();
                let _ = store.record_artifact(
                    &run_id,
                    NewArtifact {
                        kind: classify_kind(base),
                        remote_path: remote_file.clone(),
                        local_path: local.to_str().map(str::to_string),
                        size_bytes: size,
                        sha256: sha256_of_file(&local).ok(),
                        is_best: false,
                    },
                );
            }
        }
        self.append_event(
            "pull",
            "ok",
            Some(format!("matched {pattern}: {} files", files.len())),
        );
        Ok(())
    }

    fn process_alive(&self, h: &InstanceHandle) -> Option<bool> {
        let run_id = self.run_id().ok()?;
        let session = self.session_of(h).ok()?;
        let pid_file = format!("{}/run.pid", self.run_dir_for(h, &run_id));
        let out = self
            .kernel(&session, &cmd::alive_snippet(&pid_file), PROBE_TIMEOUT)
            .ok()?;
        cmd::parse_alive(&out).ok().flatten()
    }

    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let session = self.session_of(h)?;
        let run_dir = self.run_dir_for(h, &run_id);

        // Stop the training process first so it can flush; a failure here
        // (e.g. the session is already gone) must not keep the session alive,
        // releasing it kills everything anyway.
        if let Err(e) = self.sh_ok_once(&session, &cmd::kill_script(&run_dir), SH_TIMEOUT) {
            tracing::warn!("colab destroy: kill step failed ({e}); releasing session anyway");
        }
        self.bridge.session_stop(&session)?;

        if let Some(store) = self.store.borrow_mut().as_mut() {
            let _ = store.update_instance_destroyed(&h.id, Utc::now());
        }
        self.append_event("instance_destroyed", "ok", None);
        Ok(())
    }
}
