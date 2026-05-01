#![deny(unsafe_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};
use xrun_core::{
    config::credentials::SshHostCredentials,
    error::VendorError,
    manifest::{validate as core_validate, DataSource, Manifest, RunSpec, Vendor},
    store::{NewArtifact, NewEvent, RunId, Store},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter, VendorRemoteInstance, VendorStatus},
};

use crate::cmd::{self, SshConn};
use crate::error::SshError;
use crate::ssh::{remote_file_size, remote_tail, rsync, ssh_exec};

/// SSH adapter — runs the manifest on a long-lived remote machine reachable
/// over SSH. Always-on hardware: `provision`/`destroy` only manage per-run
/// state, never the box itself.
pub struct SshAdapter {
    store: RefCell<Option<Store>>,
    run_id: RefCell<Option<RunId>>,
    /// Resolved connection details (alias + host/user/port/key).
    conn: SshConn,
    /// Per-run remote workdir root, e.g. `/tmp/xrun`. The actual run dir is
    /// `<workdir_root>/<run_id>/`.
    workdir_root: String,
    /// Optional `CUDA_VISIBLE_DEVICES` value to forward through the env.
    gpu_hint: RefCell<Option<String>>,
}

impl SshAdapter {
    /// Construct from already-resolved connection details and a manifest's
    /// `ssh.workdir`. Use `from_credentials` in production to look up the
    /// connection by alias.
    pub fn new(store: Store, conn: SshConn, workdir_root: String) -> Self {
        Self {
            store: RefCell::new(Some(store)),
            run_id: RefCell::new(None),
            conn,
            workdir_root,
            gpu_hint: RefCell::new(None),
        }
    }

    /// Resolve a host alias from `[vendors.ssh.<alias>]` into a `SshConn`.
    /// Required fields: `host`, `user`. `port` defaults to 22, `key` is
    /// passed through verbatim (tilde-expansion happens in `ssh.rs`).
    pub fn resolve_conn(alias: &str, host_creds: &SshHostCredentials) -> Result<SshConn, SshError> {
        let host = host_creds.host.clone().ok_or(SshError::HostFieldMissing {
            alias: alias.to_string(),
            field: "host",
        })?;
        let user = host_creds.user.clone().ok_or(SshError::HostFieldMissing {
            alias: alias.to_string(),
            field: "user",
        })?;
        let port = host_creds.port.unwrap_or(22);
        let key = host_creds
            .key
            .as_deref()
            .map(|k| PathBuf::from(expand_tilde(k)));
        Ok(SshConn {
            alias: alias.to_string(),
            host,
            user,
            port,
            key,
        })
    }

    fn run_id(&self) -> Result<RunId, VendorError> {
        self.run_id
            .borrow()
            .clone()
            .ok_or_else(|| VendorError::Other("SshAdapter: run_id not set".into()))
    }

    fn run_dir(&self, run_id: &RunId) -> String {
        format!("{}/{}", self.workdir_root.trim_end_matches('/'), run_id)
    }

    fn instance_id(&self, run_id: &RunId) -> String {
        format!("ssh-{}-{run_id}", self.conn.alias)
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

impl VendorAdapter for SshAdapter {
    fn name(&self) -> &'static str {
        "ssh"
    }

    fn set_run_id(&self, run_id: &RunId) {
        *self.run_id.borrow_mut() = Some(run_id.clone());
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        core_validate(manifest)?;
        if !matches!(manifest.vendor, Vendor::Ssh) {
            return Err(VendorError::Validation(format!(
                "SshAdapter requires vendor=ssh, got {:?}",
                manifest.vendor
            )));
        }
        if manifest.run.cmd.is_none() {
            return Err(VendorError::Validation(
                "vendor=ssh requires run.cmd (notebooks not supported)".into(),
            ));
        }
        Ok(())
    }

    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError> {
        self.validate(manifest)?;

        let gpu_query = manifest
            .ssh
            .as_ref()
            .and_then(|s| s.gpu.clone())
            .unwrap_or_else(|| format!("ssh:{}", self.conn.alias));

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
        let now = Utc::now();
        // Best-effort GPU probe over ssh — failure → still "connected: false".
        match ssh_exec(
            &self.conn,
            "nvidia-smi --query-gpu=name,memory.free --format=csv,noheader,nounits 2>/dev/null \
             || hostname",
        ) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).trim().to_string();
                Ok(VendorStatus {
                    connected: true,
                    balance: Some(0.0),
                    currency: Some("USD".to_string()),
                    account: Some(format!("{} · {}", self.conn.target(), text)),
                    last_checked: now,
                    error: None,
                })
            }
            Err(e) => Ok(VendorStatus {
                connected: false,
                balance: None,
                currency: None,
                account: Some(self.conn.target()),
                last_checked: now,
                error: Some(e.to_string()),
            }),
        }
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
        for inst in active.into_iter().filter(|i| i.vendor == "ssh") {
            // Liveness probe via ssh `kill -0 PID`. Cheap; still skipped if
            // the run row has no associated run_id (shouldn't happen but
            // surface as `unknown`).
            let status = inst
                .run_id
                .as_deref()
                .and_then(|rid| rid.parse::<RunId>().ok())
                .map(|rid| {
                    let pid_file = format!("{}/run.pid", self.run_dir(&rid));
                    match ssh_exec(
                        &self.conn,
                        &format!(
                            "if [ -f {pf} ]; then kill -0 $(cat {pf}) && echo running || echo exited; \
                             else echo unknown; fi",
                            pf = cmd::shell_quote(&pid_file)
                        ),
                    ) {
                        Ok(b) => String::from_utf8_lossy(&b).trim().to_string(),
                        Err(_) => "unknown".to_string(),
                    }
                });
            out.push(VendorRemoteInstance {
                id: inst.id.clone(),
                gpu: inst.gpu_type.clone(),
                num_gpus: None,
                dph_total: Some(0.0),
                status,
                uptime_secs: inst
                    .created_at
                    .map(|t| (Utc::now() - t).num_seconds().max(0) as u64),
                ssh: Some(format!("{}:{}", self.conn.host, self.conn.port)),
                region: None,
            });
        }
        Ok(out)
    }

    fn provision(&self, manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        self.validate(manifest)?;
        let run_id = self.run_id()?;

        *self.gpu_hint.borrow_mut() = manifest.ssh.as_ref().and_then(|s| s.gpu.clone());

        let run_dir = self.run_dir(&run_id);
        ssh_exec(
            &self.conn,
            &format!("mkdir -p {}", cmd::shell_quote(&run_dir)),
        )
        .map_err(SshError::into_vendor)?;

        let id = self.instance_id(&run_id);
        if let Some(store) = self.store.borrow_mut().as_mut() {
            store
                .insert_instance(&id, "ssh", Some(&run_id), None, None, Utc::now())
                .map_err(|e| VendorError::Other(format!("insert instance: {e}")))?;
        }
        self.append_event("provision", "ok", Some(format!("remote workdir={run_dir}")));

        Ok(InstanceHandle {
            id,
            vendor: "ssh".to_string(),
            ssh_host: Some(self.conn.host.clone()),
            ssh_port: Some(self.conn.port),
            ssh_user: self.conn.user.clone(),
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
        for src in sources {
            // Make sure the parent dir exists on the remote so rsync doesn't
            // fail on a missing target directory.
            if let Some(parent) = std::path::Path::new(&src.dst).parent() {
                let _ = ssh_exec(
                    &self.conn,
                    &format!(
                        "mkdir -p {}",
                        cmd::shell_quote(&parent.display().to_string())
                    ),
                );
            }
            let argv = cmd::rsync_upload_argv(&self.conn, &src.src, &src.dst);
            rsync(&argv).map_err(|e| {
                let msg = format!("rsync {} -> {}: {e}", src.src, src.dst);
                self.append_event("upload", "fail", Some(msg.clone()));
                VendorError::Other(msg)
            })?;
        }
        self.append_event("upload", "ok", None);
        Ok(())
    }

    fn execute(&self, _h: &InstanceHandle, run_spec: &RunSpec) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let run_dir = self.run_dir(&run_id);

        let workdir = run_spec.workdir.clone().unwrap_or_else(|| run_dir.clone());

        ssh_exec(
            &self.conn,
            &format!("mkdir -p {}", cmd::shell_quote(&workdir)),
        )
        .map_err(SshError::into_vendor)?;

        // Run setup synchronously (errors out hard) before backgrounding the
        // main command. cd into workdir for both.
        if let Some(setup) = run_spec.setup.as_deref().filter(|s| !s.trim().is_empty()) {
            self.append_event("env_ready", "start", None);
            let setup_remote = format!(
                "cd {wd} && ({sx})",
                wd = cmd::shell_quote(&workdir),
                sx = setup
            );
            ssh_exec(&self.conn, &setup_remote).map_err(|e| {
                self.append_event("env_ready", "fail", Some(e.to_string()));
                VendorError::Other(format!("setup failed: {e}"))
            })?;
            self.append_event("env_ready", "ok", None);
        }

        // Build the user command with run.args appended, env exports prefixed.
        let cmd_base = run_spec
            .cmd
            .as_deref()
            .ok_or_else(|| VendorError::Validation("run.cmd required for ssh".into()))?;
        let user_cmd = build_cmd_line(cmd_base, run_spec);
        let env_prefix = self.env_prefix(&run_id, &run_dir);
        let full_cmd = format!(
            "cd {wd} && {env}{user}",
            wd = cmd::shell_quote(&workdir),
            env = env_prefix,
            user = user_cmd
        );

        // Background, redirect, capture PID.
        ssh_exec(&self.conn, &cmd::remote_launch_script(&run_dir, &full_cmd))
            .map_err(SshError::into_vendor)?;

        // Pull the recorded PID back so we can write a useful event.
        let pid_bytes = ssh_exec(
            &self.conn,
            &format!("cat {}/run.pid", cmd::shell_quote(&run_dir)),
        )
        .map_err(SshError::into_vendor)?;
        let pid_str = String::from_utf8_lossy(&pid_bytes).trim().to_string();
        self.append_event(
            "train_start",
            "ok",
            Some(format!("pid={pid_str} host={}", self.conn.target())),
        );
        Ok(())
    }

    fn tail(&self, _h: &InstanceHandle, file: &str, offset: u64) -> Result<Vec<u8>, VendorError> {
        // Match xrun-vast: probe the file size first so we can detect a
        // truncation (pre-emption restart on the remote) and emit the
        // correct error variant for the poller's recovery path.
        let size = remote_file_size(&self.conn, file).map_err(SshError::into_vendor)?;
        if size < offset {
            return Err(VendorError::Truncated);
        }
        if size == offset {
            return Ok(Vec::new());
        }
        remote_tail(&self.conn, file, offset).map_err(SshError::into_vendor)
    }

    fn pull(&self, _h: &InstanceHandle, remote: &str, into: &Path) -> Result<(), VendorError> {
        let run_id = self.run_id()?;
        let workdir = self.run_dir(&run_id);
        // Resolve the pattern remote-side. If it's relative, anchor at the
        // run dir; if absolute, pass through as-is.
        let pattern = if remote.starts_with('/') {
            remote.to_string()
        } else {
            format!("{workdir}/{remote}")
        };
        std::fs::create_dir_all(into)
            .map_err(|e| VendorError::Other(format!("create local pull dir: {e}")))?;
        let argv = cmd::rsync_download_argv(&self.conn, &pattern, &into.display().to_string());
        rsync(&argv).map_err(|e| VendorError::Other(format!("rsync pull {pattern}: {e}")))?;

        // Record artifact rows for whatever rsync just dropped into `into`.
        if let Some(store) = self.store.borrow_mut().as_mut() {
            if let Ok(entries) = std::fs::read_dir(into) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if !meta.is_file() {
                            continue;
                        }
                        let local_path = entry.path();
                        let kind = classify_kind(
                            local_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(""),
                        );
                        let sha = sha256_of_file(&local_path).ok();
                        let _ = store.record_artifact(
                            &run_id,
                            NewArtifact {
                                kind,
                                remote_path: pattern.clone(),
                                local_path: local_path.to_str().map(str::to_string),
                                size_bytes: Some(meta.len() as i64),
                                sha256: sha,
                                is_best: false,
                            },
                        );
                    }
                }
            }
        }
        self.append_event("pull", "ok", Some(format!("matched {pattern}")));
        Ok(())
    }

    fn process_alive(&self, _h: &InstanceHandle) -> Option<bool> {
        let run_id = self.run_id().ok()?;
        let pid_file = format!("{}/run.pid", self.run_dir(&run_id));
        let probe = format!(
            "if [ -f {pf} ]; then PID=$(cat {pf}); \
             if kill -0 \"$PID\" 2>/dev/null; then echo alive; else echo dead; fi; \
             else echo no_pid; fi",
            pf = cmd::shell_quote(&pid_file)
        );
        let bytes = ssh_exec(&self.conn, &probe).ok()?;
        match String::from_utf8_lossy(&bytes).trim() {
            "alive" => Some(true),
            "dead" => Some(false),
            _ => None,
        }
    }

    fn destroy(&self, h: &InstanceHandle) -> Result<(), VendorError> {
        // Best-effort: kill the child PID, clean its state, never the box.
        if let Ok(run_id) = self.run_id() {
            let run_dir = self.run_dir(&run_id);
            let pid_file = format!("{run_dir}/run.pid");
            let kill_script = format!(
                "if [ -f {pf} ]; then PID=$(cat {pf}); kill -TERM \"$PID\" 2>/dev/null; \
                 sleep 1; kill -KILL \"$PID\" 2>/dev/null; rm -f {pf}; fi",
                pf = cmd::shell_quote(&pid_file)
            );
            let _ = ssh_exec(&self.conn, &kill_script);
        }
        if let Some(store) = self.store.borrow_mut().as_mut() {
            let _ = store.update_instance_destroyed(&h.id, Utc::now());
        }
        self.append_event("instance_destroyed", "ok", None);
        Ok(())
    }
}

impl SshAdapter {
    fn env_prefix(&self, run_id: &RunId, run_dir: &str) -> String {
        let mut parts = vec![
            format!("XRUN_RUN_ID={}", cmd::shell_quote(&run_id.to_string())),
            format!("XRUN_RUN_DIR={}", cmd::shell_quote(run_dir)),
        ];
        if let Some(gpu) = self.gpu_hint.borrow().as_deref() {
            match gpu {
                "auto" | "" => {}
                "cpu" => parts.push("CUDA_VISIBLE_DEVICES=".to_string()),
                other => {
                    let stripped = other.strip_prefix("cuda:").unwrap_or(other);
                    parts.push(format!(
                        "CUDA_VISIBLE_DEVICES={}",
                        cmd::shell_quote(stripped)
                    ));
                }
            }
        }
        let mut prefix = parts.join(" ");
        if !prefix.is_empty() {
            prefix.push(' ');
        }
        prefix
    }
}

trait SshErrorExt {
    fn into_vendor(self) -> VendorError;
}

impl SshErrorExt for SshError {
    fn into_vendor(self) -> VendorError {
        self.into()
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

fn classify_kind(filename: &str) -> String {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "pt" | "ckpt" => "checkpoint",
        "png" | "jpg" | "jpeg" | "svg" => "figure",
        "json" => "json",
        "log" => "log",
        _ => "other",
    }
    .to_string()
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
    let mut s = String::with_capacity(64);
    for b in hasher.finalize() {
        s.push_str(&format!("{b:02x}"));
    }
    Ok(s)
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new() {
            return home.home_dir().join(rest).display().to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrun_core::manifest::Manifest;

    fn fake_conn() -> SshConn {
        SshConn {
            alias: "ws".to_string(),
            host: "10.0.0.1".to_string(),
            user: "ubuntu".to_string(),
            port: 22,
            key: None,
        }
    }

    fn ssh_manifest(extra: &str) -> Manifest {
        let yaml = format!(
            r#"
name: ssh-test
vendor: ssh
ssh:
  host_alias: ws
{extra}
run:
  cmd: echo hi
"#
        );
        Manifest::from_yaml_str(&yaml).expect("parse")
    }

    fn fresh_store() -> (tempfile::TempDir, Store) {
        let td = tempfile::TempDir::new().unwrap();
        let s = Store::open(&td.path().join("runs.db")).unwrap();
        (td, s)
    }

    #[test]
    fn name_is_ssh() {
        let (_td, s) = fresh_store();
        let a = SshAdapter::new(s, fake_conn(), "/tmp/xrun".into());
        assert_eq!(a.name(), "ssh");
    }

    #[test]
    fn validate_accepts_ssh_manifest() {
        let (_td, s) = fresh_store();
        let a = SshAdapter::new(s, fake_conn(), "/tmp/xrun".into());
        a.validate(&ssh_manifest("")).expect("valid");
    }

    #[test]
    fn validate_rejects_non_ssh_vendor() {
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
        let (_td, s) = fresh_store();
        let a = SshAdapter::new(s, fake_conn(), "/tmp/xrun".into());
        let err = a.validate(&m).expect_err("must reject");
        assert!(matches!(err, VendorError::Validation(_)));
    }

    #[test]
    fn dry_run_plan_uses_alias_in_gpu_query() {
        let (_td, s) = fresh_store();
        let a = SshAdapter::new(s, fake_conn(), "/tmp/xrun".into());
        let plan = a.dry_run_plan(&ssh_manifest("")).expect("plan");
        assert_eq!(plan.gpu_query, "ssh:ws");
        assert_eq!(plan.estimated_price_max, 0.0);
        assert!(plan.cmd_line.contains("echo hi"));
    }

    #[test]
    fn resolve_conn_requires_host_and_user() {
        let creds = SshHostCredentials {
            host: None,
            user: Some("u".to_string()),
            ..Default::default()
        };
        let err = SshAdapter::resolve_conn("alias", &creds).expect_err("missing host");
        assert!(matches!(
            err,
            SshError::HostFieldMissing { field: "host", .. }
        ));
    }

    #[test]
    fn resolve_conn_default_port_22() {
        let creds = SshHostCredentials {
            host: Some("h".to_string()),
            user: Some("u".to_string()),
            port: None,
            key: None,
            default_workdir: None,
        };
        let conn = SshAdapter::resolve_conn("alias", &creds).expect("ok");
        assert_eq!(conn.port, 22);
    }

    #[test]
    fn env_prefix_handles_cpu_and_cuda_hints() {
        let (_td, s) = fresh_store();
        let a = SshAdapter::new(s, fake_conn(), "/tmp/xrun".into());
        let rid: RunId = ulid::Ulid::new().to_string().parse().unwrap();
        // No hint → no CUDA var.
        let env = a.env_prefix(&rid, "/tmp/xrun/abc");
        assert!(env.contains("XRUN_RUN_ID="));
        assert!(env.contains("XRUN_RUN_DIR="));
        assert!(!env.contains("CUDA_VISIBLE_DEVICES"));

        *a.gpu_hint.borrow_mut() = Some("cpu".to_string());
        assert!(a
            .env_prefix(&rid, "/tmp/xrun/abc")
            .contains("CUDA_VISIBLE_DEVICES="));

        *a.gpu_hint.borrow_mut() = Some("cuda:0".to_string());
        let env = a.env_prefix(&rid, "/tmp/xrun/abc");
        assert!(env.contains("CUDA_VISIBLE_DEVICES='0'"));
    }
}
