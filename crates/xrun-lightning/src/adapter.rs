#![deny(unsafe_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};
use xrun_core::{
    config::credentials::LightningCredentials,
    error::VendorError,
    manifest::{validate as core_validate, DataSource, Manifest, RunSpec, Vendor},
    store::{NewArtifact, NewEvent, RunId, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter, VendorRemoteInstance, VendorStatus},
};
use xrun_ssh::{
    adapter::home_relative, build_cmd_line, classify_kind, effective_run_dir, pull_pattern,
    remote_run_dir, training_dir,
};

use crate::bridge::{LightningBridge, PyLightningBridge, StartParams, StudioRef, T_QUICK, T_SETUP};
use crate::cmd;
use crate::error::LightningError;

const DEFAULT_WORKDIR: &str = "xrun";
const DEFAULT_MACHINE: &str = "T4";

/// Lightning AI adapter: a Studio is the box. Runs share the ssh layout
/// (`<workdir>/<run_id>/{events.jsonl,metrics.jsonl,stdout.log,run.pid}`).
///
/// The studio name and teamspace live in the [`InstanceHandle`]
/// (`ssh_host` / `ssh_user`), so later ticks never need the manifest.
pub struct LightningAdapter {
    store: RefCell<Option<Store>>,
    run_id: RefCell<Option<RunId>>,
    bridge: Box<dyn LightningBridge>,
    /// Teamspace from credentials (a manifest's `lightning.teamspace` wins).
    default_teamspace: Option<String>,
    /// Home-relative remote root; refined from the manifest at provision.
    workdir_root: RefCell<String>,
    gpu_hint: RefCell<Option<String>>,
    /// Start parameters of the last validated manifest (for `ensure_running`).
    start_params: RefCell<Option<StartParams>>,
}

impl LightningAdapter {
    /// Production constructor: the Python bridge is spawned lazily on first use.
    pub fn new(store: Store, creds: LightningCredentials) -> Self {
        let bridge = PyLightningBridge::new(&creds);
        Self::with_bridge(store, creds.teamspace, Box::new(bridge))
    }

    /// Custom bridge (tests, or a shared one). `default_teamspace` is the
    /// credentials teamspace, if any.
    pub fn with_bridge(
        store: Store,
        default_teamspace: Option<String>,
        bridge: Box<dyn LightningBridge>,
    ) -> Self {
        Self {
            store: RefCell::new(Some(store)),
            run_id: RefCell::new(None),
            bridge,
            default_teamspace: default_teamspace.filter(|t| !t.trim().is_empty()),
            workdir_root: RefCell::new(DEFAULT_WORKDIR.to_string()),
            gpu_hint: RefCell::new(None),
            start_params: RefCell::new(None),
        }
    }

    /// Remote root (`lightning.workdir`), home-relative. Used when a handle
    /// saved by an older binary has no `run_dir`.
    pub fn with_workdir_root(self, root: &str) -> Self {
        *self.workdir_root.borrow_mut() = root_from_spec(Some(root));
        self
    }

    fn run_id(&self) -> Result<RunId, VendorError> {
        self.run_id
            .borrow()
            .clone()
            .ok_or_else(|| VendorError::Other("LightningAdapter: run_id not set".into()))
    }

    fn run_dir_for(&self, h: &InstanceHandle, run_id: &RunId) -> String {
        effective_run_dir(
            h.run_dir.as_deref(),
            &self.workdir_root.borrow(),
            &run_id.to_string(),
        )
    }

    fn studio_ref(h: &InstanceHandle) -> Result<StudioRef, VendorError> {
        let name = h
            .ssh_host
            .clone()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| VendorError::Other("handle has no studio name".into()))?;
        Ok(StudioRef {
            name,
            teamspace: Some(h.ssh_user.clone()).filter(|t| !t.is_empty()),
        })
    }

    fn teamspace_for(&self, manifest: &Manifest) -> Option<String> {
        manifest
            .lightning
            .as_ref()
            .and_then(|l| l.teamspace.clone())
            .filter(|t| !t.trim().is_empty())
            .or_else(|| self.default_teamspace.clone())
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

    /// `run` that turns a non-zero exit into an error carrying the output tail.
    fn run_ok(
        &self,
        s: &StudioRef,
        script: &str,
        timeout: std::time::Duration,
    ) -> Result<String, LightningError> {
        let out = self.bridge.run(s, script, timeout)?;
        if out.exit_code != 0 {
            return Err(LightningError::RemoteExit {
                exit_code: out.exit_code,
                output: tail_chars(&out.output, 800),
            });
        }
        Ok(out.output)
    }

    /// [`Self::run_ok`] for non-idempotent scripts: never replayed by the bridge.
    fn run_ok_once(
        &self,
        s: &StudioRef,
        script: &str,
        timeout: std::time::Duration,
    ) -> Result<String, LightningError> {
        let out = self.bridge.run_once(s, script, timeout)?;
        if out.exit_code != 0 {
            return Err(LightningError::RemoteExit {
                exit_code: out.exit_code,
                output: tail_chars(&out.output, 800),
            });
        }
        Ok(out.output)
    }

    /// Start the Studio again when it is not running (`--reuse-instance`
    /// skips `provision`; a Studio may also have been stopped or reclaimed
    /// since). `studio_start` is idempotent for a running Studio.
    fn ensure_running(&self, studio: &StudioRef) -> Result<(), VendorError> {
        let status = self.bridge.studio_status(studio)?;
        if status_is_running(&status) {
            return Ok(());
        }
        let params = self
            .start_params
            .borrow()
            .clone()
            .unwrap_or_else(|| StartParams {
                machine: DEFAULT_MACHINE.to_string(),
                interruptible: true,
                max_runtime: None,
            });
        self.append_event(
            "provision",
            "start",
            Some(format!("studio {} is {status}: restarting", studio.name)),
        );
        self.bridge.studio_start(studio, &params).map_err(|e| {
            self.append_event("provision", "fail", Some(e.to_string()));
            VendorError::from(e)
        })?;
        Ok(())
    }
}

fn root_from_spec(workdir: Option<&str>) -> String {
    let w = workdir
        .map(home_relative)
        .map(|w| w.trim_end_matches('/').to_string())
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| DEFAULT_WORKDIR.to_string());
    w
}

fn tail_chars(s: &str, n: usize) -> String {
    let t = s.trim();
    let count = t.chars().count();
    if count <= n {
        t.to_string()
    } else {
        t.chars().skip(count - n).collect()
    }
}

fn machine_of(manifest: &Manifest) -> String {
    manifest
        .lightning
        .as_ref()
        .and_then(|l| l.machine.clone())
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MACHINE.to_string())
}

fn start_params_of(manifest: &Manifest) -> StartParams {
    let spec = manifest.lightning.clone().unwrap_or_default();
    StartParams {
        machine: machine_of(manifest),
        interruptible: spec.interruptible.unwrap_or(true),
        max_runtime: spec.max_runtime_secs,
    }
}

fn status_is_running(status: &str) -> bool {
    status.trim().eq_ignore_ascii_case("running")
}

/// Remote path as the SDK wants it: relative to the studio home.
fn upload_dst(dst: &str) -> String {
    let d = home_relative(dst);
    if d == "." {
        String::new()
    } else {
        d
    }
}

impl VendorAdapter for LightningAdapter {
    fn name(&self) -> &'static str {
        "lightning"
    }

    fn set_run_id(&self, run_id: &RunId) {
        *self.run_id.borrow_mut() = Some(run_id.clone());
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        core_validate(manifest)?;
        // Launch always validates before upload/execute; `ensure_running`
        // needs these when `--reuse-instance` skipped `provision`.
        *self.start_params.borrow_mut() = Some(start_params_of(manifest));
        if !matches!(manifest.vendor, Vendor::Lightning) {
            return Err(VendorError::Validation(format!(
                "LightningAdapter requires vendor=lightning, got {:?}",
                manifest.vendor
            )));
        }
        if manifest.run.cmd.is_none() {
            return Err(VendorError::Validation(
                "vendor=lightning requires run.cmd (notebooks not supported)".into(),
            ));
        }
        for d in manifest.data.iter().flatten() {
            if d.dst.trim().starts_with('/') {
                return Err(VendorError::Validation(format!(
                    "vendor=lightning: data dst '{}' must be relative to the studio home \
                     (e.g. 'data/train.h5' or '~/data/train.h5')",
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
            gpu_query: machine_of(manifest),
            estimated_price_max: 0.0,
            data_total_bytes,
            data_items,
            cmd_line: build_cmd_line(cmd_base, &manifest.run),
        })
    }

    fn vendor_status(&self) -> Result<VendorStatus, VendorError> {
        let now = Utc::now();
        match self.bridge.whoami(self.default_teamspace.as_deref()) {
            Ok(w) => Ok(VendorStatus {
                connected: true,
                balance: None,
                currency: None,
                account: Some(match w.teamspace {
                    Some(ts) => format!("{} · {ts}", w.user),
                    None => w.user,
                }),
                last_checked: now,
                error: None,
            }),
            Err(e) => Ok(VendorStatus {
                connected: false,
                balance: None,
                currency: None,
                account: None,
                last_checked: now,
                error: Some(e.to_string()),
            }),
        }
    }

    fn vendor_instances(&self) -> Result<Vec<VendorRemoteInstance>, VendorError> {
        let active = {
            let slot = self.store.borrow();
            let Some(store) = slot.as_ref() else {
                return Ok(Vec::new());
            };
            store
                .list_active_instances()
                .map_err(|e| VendorError::Other(format!("list active: {e}")))?
        };
        let mut out = Vec::new();
        for inst in active.into_iter().filter(|i| i.vendor == "lightning") {
            let handle = inst
                .state_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<InstanceHandle>(s).ok());
            // The handle is persisted by launch after provision; before that
            // the studio name is recoverable from the id `lightning-<studio>-<run_id>`.
            let from_id = || {
                let rest = inst.id.strip_prefix("lightning-")?;
                let cut = rest.len().checked_sub(27)?; // "-" + 26-char ULID
                rest.get(..cut)
                    .filter(|n| !n.is_empty())
                    .map(|n| StudioRef {
                        name: n.to_string(),
                        teamspace: None,
                    })
            };
            let sref = handle
                .as_ref()
                .and_then(|h| Self::studio_ref(h).ok())
                .or_else(from_id);
            let (status, studio) = match sref {
                Some(s) => (
                    self.bridge
                        .studio_status(&s)
                        .map(|st| st.to_lowercase())
                        .unwrap_or_else(|_| "unknown".to_string()),
                    Some(s.name),
                ),
                _ => ("unknown".to_string(), None),
            };
            out.push(VendorRemoteInstance {
                id: inst.id.clone(),
                gpu: inst.gpu_type.clone(),
                num_gpus: None,
                dph_total: Some(0.0),
                status: Some(status),
                uptime_secs: inst
                    .created_at
                    .map(|t| (Utc::now() - t).num_seconds().max(0) as u64),
                ssh: studio,
                region: None,
            });
        }
        Ok(out)
    }

    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        self.validate(manifest)?;
        let run_id = self.run_id()?;
        let spec = manifest.lightning.clone().unwrap_or_default();

        *self.gpu_hint.borrow_mut() = spec.gpu.clone();
        let root = root_from_spec(spec.workdir.as_deref());
        *self.workdir_root.borrow_mut() = root.clone();
        let run_dir = remote_run_dir(&root, &run_id.to_string());

        let studio = StudioRef {
            name: cmd::studio_name_for(manifest),
            teamspace: self.teamspace_for(manifest),
        };
        let machine = machine_of(manifest);
        self.append_event(
            "provision",
            "start",
            Some(format!("studio={} machine={machine}", studio.name)),
        );
        let params = start_params_of(manifest);
        let info = self.bridge.studio_start(&studio, &params).map_err(|e| {
            self.append_event("provision", "fail", Some(e.to_string()));
            VendorError::from(e)
        })?;
        // Keep the teamspace the SDK resolved so later calls address the same one.
        let teamspace = studio.teamspace.clone().or(info.teamspace.clone());
        let studio = StudioRef {
            name: studio.name,
            teamspace: teamspace.clone(),
        };

        self.run_ok(&studio, &cmd::mkdir_script(&run_dir), T_QUICK)
            .map_err(|e| {
                self.append_event("provision", "fail", Some(e.to_string()));
                VendorError::from(e)
            })?;

        let id = format!("lightning-{}-{run_id}", studio.name);
        if let Some(store) = self.store.borrow_mut().as_mut() {
            store
                .insert_instance(
                    &id,
                    "lightning",
                    Some(&run_id),
                    Some(&machine),
                    None,
                    Utc::now(),
                )
                .map_err(|e| VendorError::Other(format!("insert instance: {e}")))?;
        }
        self.append_event(
            "provision",
            "ok",
            Some(format!(
                "studio={} machine={machine} workdir={run_dir}",
                studio.name
            )),
        );
        Ok(InstanceHandle {
            id,
            vendor: "lightning".to_string(),
            ssh_host: Some(studio.name),
            ssh_port: None,
            ssh_user: teamspace.unwrap_or_default(),
            run_dir: Some(run_dir),
        })
    }

    fn upload(&self, h: &InstanceHandle, sources: &[DataSource]) -> Result<(), VendorError> {
        if sources.is_empty() {
            self.append_event("upload", "ok", Some("no data sources".into()));
            return Ok(());
        }
        let studio = Self::studio_ref(h)?;
        self.ensure_running(&studio)?;
        self.append_event(
            "upload",
            "start",
            Some(format!("{} sources", sources.len())),
        );
        for src in sources {
            let fail = |e: LightningError| {
                let msg = format!("upload {} -> {}: {e}", src.src, src.dst);
                self.append_event("upload", "fail", Some(msg.clone()));
                VendorError::Other(msg)
            };
            if src.dst.trim().starts_with('/') {
                return Err(fail(LightningError::Other(
                    "dst must be relative to the studio home".into(),
                )));
            }
            let remote = upload_dst(&src.dst);
            let local = Path::new(&src.src);
            if local.is_dir() {
                self.bridge
                    .upload_folder(&studio, local, &remote)
                    .map_err(fail)?;
            } else {
                if let Some(parent) = Path::new(&remote).parent().and_then(|p| p.to_str()) {
                    if !parent.is_empty() {
                        self.run_ok(&studio, &cmd::mkdir_script(parent), T_QUICK)
                            .map_err(fail)?;
                    }
                }
                self.bridge
                    .upload_file(&studio, local, &remote)
                    .map_err(fail)?;
            }
        }
        self.append_event("upload", "ok", None);
        Ok(())
    }

    fn execute(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let studio = Self::studio_ref(h)?;
        self.ensure_running(&studio)?;
        let run_dir = self.run_dir_for(h, &run_id);
        let workdir = training_dir(run_spec, &run_dir);

        if let Some(setup) = run_spec.setup.as_deref().filter(|s| !s.trim().is_empty()) {
            self.append_event("env_ready", "start", None);
            self.run_ok(&studio, &cmd::setup_script(&workdir, setup), T_SETUP)
                .map_err(|e| {
                    self.append_event("env_ready", "fail", Some(e.to_string()));
                    VendorError::Other(format!("setup failed: {e}"))
                })?;
            self.append_event("env_ready", "ok", None);
        }

        let cmd_base = run_spec
            .cmd
            .as_deref()
            .ok_or_else(|| VendorError::Validation("run.cmd required for lightning".into()))?;
        let user_cmd = build_cmd_line(cmd_base, run_spec);
        let env = cmd::env_exports(
            &run_id.to_string(),
            &run_dir,
            self.gpu_hint.borrow().as_deref(),
        );
        let script = cmd::launch_script(&run_dir, &workdir, &env, &user_cmd);
        // Starts the training: must not be replayed if the bridge child dies.
        let out = self
            .run_ok_once(&studio, &script, T_QUICK)
            .map_err(VendorError::from)?;
        let pid = out.lines().last().unwrap_or("").trim().to_string();
        self.append_event(
            "train_start",
            "ok",
            Some(format!("pid={pid} studio={}", studio.name)),
        );
        Ok(())
    }

    fn tail(&self, h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError> {
        let studio = Self::studio_ref(h)?;
        let out = self.run_ok(&studio, &cmd::tail_script(file, offset), T_QUICK)?;
        let (size, bytes) = cmd::parse_tail_output(&out).map_err(LightningError::Protocol)?;
        if size < offset {
            return Err(VendorError::Truncated);
        }
        if size == offset {
            return Ok(Vec::new());
        }
        Ok(bytes)
    }

    fn pull(&self, h: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let studio = Self::studio_ref(h)?;
        let run_dir = self.run_dir_for(h, &run_id);
        let pattern = pull_pattern(&run_dir, remote);
        std::fs::create_dir_all(into)
            .map_err(|e| VendorError::Other(format!("create local pull dir: {e}")))?;

        let listing = self.run_ok(&studio, &cmd::pull_script(&pattern), T_QUICK)?;
        let files = cmd::parse_pull_output(&listing);
        for rpath in &files {
            let base = rpath.rsplit('/').next().unwrap_or(rpath);
            let local = into.join(base);
            self.bridge
                .download_file(&studio, rpath, &local)
                .map_err(|e| VendorError::Other(format!("download {rpath}: {e}")))?;
            let meta = std::fs::metadata(&local)?;
            let sha = sha256_of_file(&local).ok();
            if let Some(store) = self.store.borrow_mut().as_mut() {
                let _ = store.record_artifact(
                    &run_id,
                    NewArtifact {
                        kind: classify_kind(base),
                        remote_path: rpath.clone(),
                        local_path: local.to_str().map(str::to_string),
                        size_bytes: Some(meta.len() as i64),
                        sha256: sha,
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
        let studio = Self::studio_ref(h).ok()?;
        // A reclaimed / stopped Studio means the training is gone. Only a
        // failing status call leaves the answer unknown.
        let status = self.bridge.studio_status(&studio).ok()?;
        if !status_is_running(&status) {
            return Some(false);
        }
        let script = cmd::alive_script(&self.run_dir_for(h, &run_id));
        let out = self.bridge.run(&studio, &script, T_QUICK).ok()?;
        match out.exit_code {
            0 => Some(true),
            1 => Some(false),
            _ => None,
        }
    }

    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let studio = Self::studio_ref(h)?;
        let run_dir = self.run_dir_for(h, &run_id);
        // A failed kill must not keep the studio (and its credits) running:
        // note it and release the box anyway.
        if let Err(e) = self.run_ok_once(&studio, &cmd::kill_script(&run_dir), T_QUICK) {
            self.append_event("instance_destroyed", "warn", Some(format!("kill: {e}")));
        }
        self.bridge
            .studio_stop(&studio)
            .map_err(VendorError::from)?;
        if let Some(store) = self.store.borrow_mut().as_mut() {
            let _ = store.update_instance_destroyed(&h.id, Utc::now());
        }
        self.append_event("instance_destroyed", "ok", None);
        Ok(())
    }
}

fn sha256_of_file(path: &Path) -> Result<String, std::io::Error> {
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

#[cfg(test)]
mod tests;
