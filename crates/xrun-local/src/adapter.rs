#![deny(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use xrun_core::{
    error::VendorError,
    manifest::{
        validate as core_validate, DataMode, DataSource, LocalSpec, Manifest, RunSpec, Vendor,
    },
    store::{NewArtifact, NewEvent, RunId, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter, VendorRemoteInstance, VendorStatus},
};

use crate::process::{
    kill_process, probe_gpu_summary, process_alive, run_setup_blocking, spawn_main,
};
use crate::shell::resolve_shell;
use crate::tail::tail_file;
use crate::transfer::{classify_kind, copy_path, glob_in_workdir, pull_matches};

/// Local subprocess adapter. Runs the manifest's `run.cmd` as a host process
/// with stdout/stderr captured to `<runs_dir>/<run_id>/stdout.log`.
pub struct LocalAdapter {
    store: RefCell<Option<Store>>,
    run_id: RefCell<Option<RunId>>,
    runs_dir: PathBuf,
    /// Cached at `provision()` time so `execute()` can build the right env
    /// without holding a `Manifest` reference.
    local_spec: RefCell<Option<LocalSpec>>,
    /// Cached at `execute()` time so `pull()` can resolve glob patterns
    /// against the same dir the subprocess ran in. `pull()` falls back to the
    /// launch CWD when this is unset (e.g. on `xrun pull` after a foreground
    /// run already exited).
    workdir: RefCell<Option<PathBuf>>,
}

impl LocalAdapter {
    /// Stateless constructor (no Store). Used by the dispatch in `xrun-cli` for
    /// dry-run paths and for unit tests that don't touch the DB. Provisioning
    /// without a Store still spawns processes but skips the instance row.
    pub fn new() -> Self {
        Self {
            store: RefCell::new(None),
            run_id: RefCell::new(None),
            runs_dir: PathBuf::new(),
            local_spec: RefCell::new(None),
            workdir: RefCell::new(None),
        }
    }

    pub fn with_store_and_runs_dir(store: Store, runs_dir: PathBuf) -> Self {
        Self {
            store: RefCell::new(Some(store)),
            run_id: RefCell::new(None),
            runs_dir,
            local_spec: RefCell::new(None),
            workdir: RefCell::new(None),
        }
    }

    fn run_id(&self) -> Result<RunId, VendorError> {
        self.run_id
            .borrow()
            .clone()
            .ok_or_else(|| VendorError::Other("LocalAdapter: run_id not set".into()))
    }

    fn run_dir(&self, run_id: &RunId) -> PathBuf {
        self.runs_dir.join(run_id.to_string())
    }

    fn instance_id(run_id: &RunId) -> String {
        format!("local-{run_id}")
    }

    fn append_event(&self, stage: &str, status: &str, msg: Option<String>) {
        let Ok(run_id) = self.run_id() else {
            return;
        };
        let mut store_slot = self.store.borrow_mut();
        let Some(store) = store_slot.as_mut() else {
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
}

impl Default for LocalAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl VendorAdapter for LocalAdapter {
    fn name(&self) -> &'static str {
        "local"
    }

    fn set_run_id(&self, run_id: &RunId) {
        *self.run_id.borrow_mut() = Some(run_id.clone());
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        core_validate(manifest)?;
        if !matches!(manifest.vendor, Vendor::Local) {
            return Err(VendorError::Validation(format!(
                "LocalAdapter requires vendor=local, got {:?}",
                manifest.vendor
            )));
        }
        if manifest.run.cmd.is_none() {
            return Err(VendorError::Validation(
                "vendor=local requires run.cmd (notebooks not supported)".into(),
            ));
        }
        Ok(())
    }

    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError> {
        self.validate(manifest)?;

        let gpu_query = manifest
            .local
            .as_ref()
            .and_then(|l| l.gpu.clone())
            .unwrap_or_else(|| "auto".to_string());

        let mut data_items: Vec<(PathBuf, String)> = Vec::new();
        let mut data_total_bytes: u64 = 0;
        if let Some(data) = &manifest.data {
            for source in data {
                let src = PathBuf::from(&source.src);
                let bytes = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
                data_total_bytes += bytes;
                data_items.push((src, source.dst.clone()));
            }
        }

        let cmd_base = manifest.run.cmd.as_deref().unwrap_or("");
        let cmd_line = build_cmd_line(cmd_base, &manifest.run);

        Ok(DryRunPlan {
            gpu_query,
            estimated_price_max: 0.0,
            data_total_bytes,
            data_items,
            cmd_line,
        })
    }

    fn vendor_status(&self) -> Result<VendorStatus, VendorError> {
        let host = hostname().unwrap_or_else(|| "localhost".to_string());
        let gpus = probe_gpu_summary();
        let account = if gpus.is_empty() {
            Some(host)
        } else {
            Some(format!("{host} · {}", gpus.join(" / ")))
        };
        Ok(VendorStatus {
            connected: true,
            balance: Some(0.0),
            currency: Some("USD".to_string()),
            account,
            last_checked: Utc::now(),
            error: None,
        })
    }

    fn vendor_instances(&self) -> Result<Vec<VendorRemoteInstance>, VendorError> {
        let store_slot = self.store.borrow();
        let Some(store) = store_slot.as_ref() else {
            return Ok(Vec::new());
        };
        let active = store
            .list_active_instances()
            .map_err(|e| VendorError::Other(format!("list active: {e}")))?;
        let mut out = Vec::new();
        for inst in active.into_iter().filter(|i| i.vendor == "local") {
            // For local rows the PID lives in <run_dir>/run.pid. We use the
            // instance.run_id to find the run dir; if it's gone, treat as
            // unknown but still surface the row.
            let alive_status = inst
                .run_id
                .as_deref()
                .and_then(|rid| rid.parse::<RunId>().ok())
                .map(|rid| self.read_pid(&rid))
                .map(|pid_opt| match pid_opt {
                    Some(pid) if process_alive(pid) => "running".to_string(),
                    Some(_) => "exited".to_string(),
                    None => "unknown".to_string(),
                });
            out.push(VendorRemoteInstance {
                id: inst.id.clone(),
                gpu: inst.gpu_type.clone(),
                num_gpus: None,
                dph_total: Some(0.0),
                status: alive_status,
                uptime_secs: inst
                    .created_at
                    .map(|t| (Utc::now() - t).num_seconds().max(0) as u64),
                ssh: None,
                region: None,
            });
        }
        Ok(out)
    }

    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        self.validate(manifest)?;
        let run_id = self.run_id()?;

        // Cache the local block so execute() can build the env without
        // re-reading the manifest from disk.
        *self.local_spec.borrow_mut() = manifest.local.clone();

        let run_dir = self.run_dir(&run_id);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| VendorError::Other(format!("create run_dir: {e}")))?;

        // Pre-create workdir only when the manifest set one explicitly.
        // The default (current_dir) already exists.
        let workdir = effective_workdir(&manifest.run, &run_dir);
        if manifest.run.workdir.is_some() {
            std::fs::create_dir_all(&workdir)
                .map_err(|e| VendorError::Other(format!("create workdir: {e}")))?;
        }

        let id = Self::instance_id(&run_id);
        if let Some(store) = self.store.borrow_mut().as_mut() {
            store
                .insert_instance(&id, "local", Some(&run_id), None, None, Utc::now())
                .map_err(|e| VendorError::Other(format!("insert instance: {e}")))?;
        }

        self.append_event("provision", "ok", Some(format!("workdir={workdir:?}")));

        Ok(InstanceHandle {
            id,
            vendor: "local".to_string(),
            ssh_host: None,
            ssh_port: None,
            ssh_user: String::new(),
        })
    }

    fn upload(&self, _h: &InstanceHandle, sources: &[DataSource]) -> Result<(), VendorError> {
        if sources.is_empty() {
            self.append_event("upload", "ok", Some("no data sources".into()));
            return Ok(());
        }
        self.append_event(
            "upload",
            "start",
            Some(format!("{} sources", sources.len())),
        );

        let mut total: u64 = 0;
        for src_spec in sources {
            // MVP only supports plain copy. Surface anything else as a warning
            // event but still copy the bytes so the run can proceed.
            if matches!(src_spec.mode, Some(DataMode::Rsync)) {
                self.append_event(
                    "upload",
                    "progress",
                    Some(format!(
                        "warn: rsync mode not supported for vendor=local — using copy ({})",
                        src_spec.src
                    )),
                );
            }
            if src_spec.unpack.is_some()
                || !src_spec.exclude.is_empty()
                || src_spec.compress.is_some()
            {
                self.append_event(
                    "upload",
                    "progress",
                    Some(format!(
                        "warn: unpack/exclude/compress ignored for vendor=local ({})",
                        src_spec.src
                    )),
                );
            }

            let src = std::path::PathBuf::from(&src_spec.src);
            let dst = std::path::PathBuf::from(&src_spec.dst);
            let bytes = copy_path(&src, &dst).map_err(|e| {
                let msg = format!("copy {} -> {}: {e}", src.display(), dst.display());
                self.append_event("upload", "fail", Some(msg.clone()));
                VendorError::Other(msg)
            })?;
            total = total.saturating_add(bytes);
        }

        self.append_event("upload", "ok", Some(format!("{total} bytes")));
        Ok(())
    }

    fn execute(&self, h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let run_dir = self.run_dir(&run_id);
        let workdir = effective_workdir(run_spec, &run_dir);
        if run_spec.workdir.is_some() {
            std::fs::create_dir_all(&workdir)
                .map_err(|e| VendorError::Other(format!("create workdir: {e}")))?;
        }
        *self.workdir.borrow_mut() = Some(workdir.clone());

        let stdout_path = run_dir.join("stdout.log");
        let env = build_env(&run_id, &run_dir, self.local_spec.borrow().as_ref());

        let shell = resolve_shell()?;

        if let Some(setup_script) = run_spec.setup.as_deref().filter(|s| !s.trim().is_empty()) {
            self.append_event("env_ready", "start", None);
            run_setup_blocking(&shell, setup_script, &workdir, &env).map_err(|e| {
                self.append_event("env_ready", "fail", Some(e.to_string()));
                VendorError::Other(format!("setup failed: {e}"))
            })?;
            self.append_event("env_ready", "ok", None);
        }

        let cmd_base = run_spec
            .cmd
            .as_deref()
            .ok_or_else(|| VendorError::Validation("run.cmd required for local".into()))?;
        let full_cmd = build_cmd_line(cmd_base, run_spec);

        let pid = spawn_main(&shell, &full_cmd, &workdir, &env, &stdout_path)
            .map_err(|e| VendorError::Other(format!("spawn: {e}")))?;

        // PID lives in the run dir, not in instance.state_json — launch.rs
        // puts the serialized InstanceHandle there for poll-daemon to
        // reconstruct, and we don't want to clobber it.
        let pid_file = run_dir.join("run.pid");
        if let Err(e) = std::fs::write(&pid_file, pid.to_string()) {
            tracing::warn!("could not persist PID to {pid_file:?}: {e}");
        }
        let _ = h; // handle is unused once the PID file is written

        self.append_event(
            "train_start",
            "ok",
            Some(format!("pid={pid} shell={}", shell.kind.label())),
        );

        Ok(())
    }

    fn tail(&self, _h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError> {
        Ok(tail_file(Path::new(file), offset)?)
    }

    fn pull(&self, _h: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        // Use the workdir cached at execute() time so glob patterns resolve
        // against the same dir the subprocess actually ran in. Fall back to
        // the launch CWD when execute didn't run in this process (e.g.
        // `xrun pull` after the run finished).
        let workdir = self
            .workdir
            .borrow()
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| self.run_dir(&run_id));

        let matches = glob_in_workdir(&workdir, remote)
            .map_err(|e| VendorError::Other(format!("glob {remote:?}: {e}")))?;
        if matches.is_empty() {
            self.append_event("pull", "progress", Some(format!("no matches for {remote}")));
            return Ok(());
        }

        let pulled = pull_matches(&matches, &workdir, into)
            .map_err(|e| VendorError::Other(format!("pull copy: {e}")))?;

        if let Some(store) = self.store.borrow_mut().as_mut() {
            for art in &pulled {
                let kind = classify_kind(
                    art.local_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(""),
                );
                let _ = store.record_artifact(
                    &run_id,
                    NewArtifact {
                        kind,
                        remote_path: art.remote_path.clone(),
                        local_path: art.local_path.to_str().map(str::to_string),
                        size_bytes: Some(art.size_bytes),
                        sha256: Some(art.sha256.clone()),
                        is_best: false,
                    },
                );
            }
        }
        self.append_event(
            "pull",
            "ok",
            Some(format!("{} files for {remote}", pulled.len())),
        );
        Ok(())
    }

    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        // Try to find a PID — best-effort. The two ways: the cached run_id
        // (set by callers like launch/stop/poll-daemon), or by walking the
        // instance row's run_id from the store.
        let pid = self
            .run_id()
            .ok()
            .and_then(|rid| self.read_pid(&rid))
            .or_else(|| self.read_pid_from_db(&h.id));

        if let Some(pid) = pid {
            if let Err(e) = kill_process(pid) {
                tracing::warn!("local destroy: kill {pid} failed: {e}");
            }
            // Remove the PID file so vendor_instances() stops surfacing the
            // row as "running".
            if let Ok(rid) = self.run_id() {
                let _ = std::fs::remove_file(self.run_dir(&rid).join("run.pid"));
            }
        }

        if let Some(store) = self.store.borrow_mut().as_mut() {
            let _ = store.update_instance_destroyed(&h.id, Utc::now());
        }
        self.append_event("instance_destroyed", "ok", pid.map(|p| format!("pid={p}")));
        Ok(())
    }
}

impl LocalAdapter {
    fn read_pid(&self, run_id: &RunId) -> Option<u32> {
        let pid_file = self.run_dir(run_id).join("run.pid");
        std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    }

    fn read_pid_from_db(&self, instance_id: &str) -> Option<u32> {
        let store_slot = self.store.borrow();
        let store = store_slot.as_ref()?;
        let inst = store.get_instance(instance_id).ok().flatten()?;
        let run_id_str = inst.run_id?;
        let rid: RunId = run_id_str.parse().ok()?;
        self.read_pid(&rid)
    }
}

fn build_cmd_line(cmd_base: &str, run_spec: &RunSpec) -> String {
    let Some(args) = &run_spec.args else {
        return cmd_base.to_string();
    };

    let mut sorted: Vec<_> = args.iter().collect();
    sorted.sort_by_key(|(k, _)| k.as_str());

    let parts: Vec<String> = sorted
        .into_iter()
        .map(|(k, v)| {
            let val = v
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string());
            format!("{k} {val}")
        })
        .collect();

    if parts.is_empty() {
        cmd_base.to_string()
    } else {
        format!("{cmd_base} {}", parts.join(" "))
    }
}

fn build_env(
    run_id: &RunId,
    run_dir: &Path,
    local_spec: Option<&LocalSpec>,
) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("XRUN_RUN_ID".to_string(), run_id.to_string());
    env.insert("XRUN_RUN_DIR".to_string(), run_dir.display().to_string());

    if let Some(gpu) = local_spec.and_then(|l| l.gpu.as_deref()) {
        match gpu {
            "auto" | "" => {} // leave CUDA_VISIBLE_DEVICES as inherited
            "cpu" => {
                env.insert("CUDA_VISIBLE_DEVICES".to_string(), String::new());
            }
            other => {
                // Accept "0", "0,1", or "cuda:0" — strip the "cuda:" prefix.
                let stripped = other.strip_prefix("cuda:").unwrap_or(other);
                env.insert("CUDA_VISIBLE_DEVICES".to_string(), stripped.to_string());
            }
        }
    }

    env
}

fn effective_workdir(run_spec: &RunSpec, _run_dir: &Path) -> PathBuf {
    // For local runs we want the manifest's `cmd` to find files relative to
    // wherever the user launched xrun from — the same intuition you get when
    // running the script directly. Cloud vendors put files into a synthetic
    // remote workdir; that's not how host execution feels.
    match run_spec.workdir.as_deref() {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use xrun_core::{manifest::Manifest, store::Store};

    fn local_manifest(extra: &str) -> Manifest {
        let yaml = format!(
            r#"
name: test-local
vendor: local
{extra}
run:
  cmd: echo hi
"#
        );
        Manifest::from_yaml_str(&yaml).expect("parse")
    }

    fn fresh_store() -> (TempDir, Store) {
        let td = TempDir::new().unwrap();
        let db = td.path().join("runs.db");
        let store = Store::open(&db).expect("open store");
        (td, store)
    }

    #[test]
    fn name_is_local() {
        let a = LocalAdapter::new();
        assert_eq!(a.name(), "local");
    }

    #[test]
    fn vendor_status_reports_connected() {
        let a = LocalAdapter::new();
        let s = a.vendor_status().expect("status");
        assert!(s.connected);
        assert_eq!(s.balance, Some(0.0));
    }

    #[test]
    fn dry_run_plan_default_gpu_is_auto() {
        let m = local_manifest("");
        let a = LocalAdapter::new();
        let plan = a.dry_run_plan(&m).expect("plan");
        assert_eq!(plan.gpu_query, "auto");
        assert_eq!(plan.estimated_price_max, 0.0);
        assert!(plan.cmd_line.contains("echo hi"));
    }

    #[test]
    fn dry_run_plan_uses_local_gpu_hint() {
        let m = local_manifest("local:\n  gpu: '0'");
        let a = LocalAdapter::new();
        let plan = a.dry_run_plan(&m).expect("plan");
        assert_eq!(plan.gpu_query, "0");
    }

    #[test]
    fn validate_rejects_non_local_vendor() {
        let yaml = r#"
name: wrong
vendor: vast
vast:
  image: foo
  gpu:
    type: RTX_4090
    count: 1
run:
  cmd: echo hi
"#;
        let m: Manifest = Manifest::from_yaml_str(yaml).expect("parse");
        let a = LocalAdapter::new();
        let err = a.validate(&m).expect_err("must reject");
        assert!(matches!(err, VendorError::Validation(_)));
    }

    #[test]
    fn validate_requires_cmd() {
        let yaml = r#"
name: nocmd
vendor: local
run: {}
"#;
        let m: Manifest = Manifest::from_yaml_str(yaml).expect("parse");
        let a = LocalAdapter::new();
        let err = a.validate(&m).expect_err("must reject");
        assert!(matches!(err, VendorError::Validation(_)));
    }

    #[test]
    fn build_env_sets_xrun_dirs() {
        let td = TempDir::new().unwrap();
        let run_id_str = ulid::Ulid::new().to_string();
        let run_id: RunId = run_id_str.parse().expect("ulid -> RunId");
        let env = build_env(&run_id, td.path(), None);
        assert_eq!(
            env.get("XRUN_RUN_ID").map(|s| s.as_str()),
            Some(run_id_str.as_str())
        );
        assert_eq!(
            env.get("XRUN_RUN_DIR").map(|s| s.as_str()),
            Some(td.path().display().to_string().as_str())
        );
        assert!(!env.contains_key("CUDA_VISIBLE_DEVICES"));
    }

    #[test]
    fn build_env_translates_cpu_gpu_hint() {
        let td = TempDir::new().unwrap();
        let run_id: RunId = ulid::Ulid::new().to_string().parse().unwrap();
        let spec = LocalSpec {
            gpu: Some("cpu".into()),
        };
        let env = build_env(&run_id, td.path(), Some(&spec));
        assert_eq!(env.get("CUDA_VISIBLE_DEVICES"), Some(&String::new()));
    }

    #[test]
    fn build_env_strips_cuda_prefix() {
        let td = TempDir::new().unwrap();
        let run_id: RunId = ulid::Ulid::new().to_string().parse().unwrap();
        let spec = LocalSpec {
            gpu: Some("cuda:0".into()),
        };
        let env = build_env(&run_id, td.path(), Some(&spec));
        assert_eq!(env.get("CUDA_VISIBLE_DEVICES"), Some(&"0".to_string()));
    }

    #[test]
    fn provision_creates_run_dir_only() {
        let (td, store) = fresh_store();
        let runs_dir = td.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let mut store = store;
        let run_id = store
            .create_run("test", "abc123", "manifest.yaml", "local", &[])
            .expect("create_run");

        let adapter = LocalAdapter::with_store_and_runs_dir(store, runs_dir.clone());
        adapter.set_run_id(&run_id);

        let m = local_manifest("");
        let h = adapter.provision(&m).expect("provision");
        assert_eq!(h.vendor, "local");
        assert_eq!(h.id, format!("local-{run_id}"));

        let run_dir = runs_dir.join(run_id.to_string());
        assert!(run_dir.is_dir(), "run_dir created: {run_dir:?}");
        // No `work/` subdir — workdir defaults to the launch CWD now.
        assert!(!run_dir.join("work").exists());
    }

    #[test]
    fn provision_creates_explicit_workdir() {
        let (td, store) = fresh_store();
        let runs_dir = td.path().join("runs");
        let staging = td.path().join("staging");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let mut store = store;
        let run_id = store
            .create_run("test2", "abc", "manifest.yaml", "local", &[])
            .unwrap();

        let yaml = format!(
            r#"
name: test-explicit
vendor: local
run:
  workdir: {}
  cmd: echo hi
"#,
            staging.display()
        );
        let m: Manifest = Manifest::from_yaml_str(&yaml).expect("parse");

        let adapter = LocalAdapter::with_store_and_runs_dir(store, runs_dir);
        adapter.set_run_id(&run_id);
        adapter.provision(&m).expect("provision");
        assert!(staging.is_dir(), "explicit workdir was created");
    }

    #[test]
    fn upload_copies_files_and_directories() {
        use xrun_core::manifest::DataSource;

        let (td, store) = fresh_store();
        let runs_dir = td.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let mut store = store;
        let run_id = store
            .create_run("up", "abc", "manifest.yaml", "local", &[])
            .unwrap();

        // Source files: one regular file, one nested dir.
        let src_dir = td.path().join("src");
        std::fs::create_dir_all(src_dir.join("nested")).unwrap();
        std::fs::write(src_dir.join("a.txt"), b"alpha").unwrap();
        std::fs::write(src_dir.join("nested/b.txt"), b"beta").unwrap();
        let single = td.path().join("script.py");
        std::fs::write(&single, b"print('hi')").unwrap();

        let dst_root = td.path().join("dst");

        let adapter = LocalAdapter::with_store_and_runs_dir(store, runs_dir);
        adapter.set_run_id(&run_id);

        let sources = vec![
            DataSource {
                src: src_dir.display().to_string(),
                dst: dst_root.join("data").display().to_string(),
                mode: None,
                unpack: None,
                exclude: vec![],
                compress: None,
            },
            DataSource {
                src: single.display().to_string(),
                dst: dst_root.join("script.py").display().to_string(),
                mode: None,
                unpack: None,
                exclude: vec![],
                compress: None,
            },
        ];

        let handle = InstanceHandle {
            id: "local-x".into(),
            vendor: "local".into(),
            ssh_host: None,
            ssh_port: None,
            ssh_user: String::new(),
        };
        adapter.upload(&handle, &sources).expect("upload");

        assert_eq!(
            std::fs::read(dst_root.join("data/a.txt")).unwrap(),
            b"alpha"
        );
        assert_eq!(
            std::fs::read(dst_root.join("data/nested/b.txt")).unwrap(),
            b"beta"
        );
        assert_eq!(
            std::fs::read(dst_root.join("script.py")).unwrap(),
            b"print('hi')"
        );
    }

    #[test]
    fn destroy_kills_pid_and_marks_destroyed() {
        let (td, store) = fresh_store();
        let runs_dir = td.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let mut store = store;
        let run_id = store
            .create_run("destroy-test", "abc", "manifest.yaml", "local", &[])
            .unwrap();

        // The script we spawn sleeps long enough for destroy() to win the
        // race: ~10s on a slow CI runner is plenty.
        let cmd = if cfg!(windows) {
            "Start-Sleep -Seconds 10"
        } else {
            "sleep 10"
        };
        let yaml = format!(
            r#"
name: destroy-test
vendor: local
run:
  cmd: |
    {cmd}
"#
        );
        let m: Manifest = Manifest::from_yaml_str(&yaml).expect("parse");

        let adapter = LocalAdapter::with_store_and_runs_dir(store, runs_dir.clone());
        adapter.set_run_id(&run_id);
        let handle = adapter.provision(&m).expect("provision");
        adapter.execute(&handle, &m.run).expect("execute");

        let pid_file = runs_dir.join(run_id.to_string()).join("run.pid");
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("pid file")
            .trim()
            .parse()
            .expect("pid number");
        assert!(crate::process::process_alive(pid), "child must be alive");

        adapter.destroy(&handle).expect("destroy");
        // Allow up to 2s for the OS to reap.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while crate::process::process_alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!crate::process::process_alive(pid), "child should be dead");
        assert!(!pid_file.exists(), "pid file removed");
    }

    #[test]
    fn pull_globs_and_records_artifacts() {
        let (td, store) = fresh_store();
        let runs_dir = td.path().join("runs");
        let staging = td.path().join("staging");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let mut store = store;
        let run_id = store
            .create_run("pull", "abc", "manifest.yaml", "local", &[])
            .unwrap();

        let yaml = format!(
            r#"
name: pull-test
vendor: local
run:
  workdir: {}
  cmd: echo hi
"#,
            staging.display()
        );
        let m: Manifest = Manifest::from_yaml_str(&yaml).expect("parse");

        let adapter = LocalAdapter::with_store_and_runs_dir(store, runs_dir);
        adapter.set_run_id(&run_id);
        let handle = adapter.provision(&m).expect("provision");
        std::fs::create_dir_all(staging.join("ckpt")).unwrap();
        std::fs::write(staging.join("ckpt/best_00.pt"), b"AAAA").unwrap();
        std::fs::write(staging.join("ckpt/best_01.pt"), b"BBBB").unwrap();
        std::fs::write(staging.join("ckpt/notes.txt"), b"x").unwrap();

        // Seed the cached workdir as `execute()` would. This also exercises
        // the fallback path when the run is being pulled in-process.
        adapter.execute(&handle, &m.run).expect("execute");

        let into = td.path().join("models");
        adapter
            .pull(&handle, "ckpt/best_*.pt", &into)
            .expect("pull");
        let pulled: Vec<_> = std::fs::read_dir(&into)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect();
        assert_eq!(pulled.len(), 2, "got: {pulled:?}");
    }

    #[test]
    fn execute_spawns_subprocess_and_tail_reads_stdout() {
        let (td, store) = fresh_store();
        let runs_dir = td.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let mut store = store;
        let run_id = store
            .create_run("smoke", "deadbeef", "manifest.yaml", "local", &[])
            .expect("create_run");

        let adapter = LocalAdapter::with_store_and_runs_dir(store, runs_dir.clone());
        adapter.set_run_id(&run_id);

        let m = local_manifest("");
        let handle = adapter.provision(&m).expect("provision");
        adapter.execute(&handle, &m.run).expect("execute");

        // Wait for stdout.log to appear.
        let stdout_path = runs_dir.join(run_id.to_string()).join("stdout.log");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(meta) = std::fs::metadata(&stdout_path) {
                if meta.len() > 0 {
                    break;
                }
            }
            if std::time::Instant::now() > deadline {
                panic!("stdout.log never grew at {stdout_path:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let bytes = adapter
            .tail(&handle, stdout_path.to_str().unwrap(), 0)
            .expect("tail");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("hi"), "stdout content: {text:?}");
    }
}
