#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use xrun_core::{
    manifest::{DataMode, DataSource},
    vendor::InstanceHandle,
};

use crate::{
    cli::{CopyEndpoint, InstanceId},
    error::VastError,
};

/// Returns the copy endpoints for a DataSource in copy mode.
pub fn copy_endpoints(
    instance_id: InstanceId,
    source: &DataSource,
) -> (CopyEndpoint, CopyEndpoint) {
    let src = CopyEndpoint::Local(PathBuf::from(&source.src));
    let dst = CopyEndpoint::Remote {
        instance: instance_id,
        path: source.dst.clone(),
    };
    (src, dst)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Returns the shell commands to run on the remote instance for unpacking, if any.
/// Returns an empty vec if no unpack spec is set.
pub fn unpack_commands(source: &DataSource) -> Result<Vec<String>, VastError> {
    let unpack = match &source.unpack {
        None => return Ok(vec![]),
        Some(u) => u,
    };

    let dst = shell_quote(&source.dst);
    let into = shell_quote(&unpack.into);

    let mkdir_cmd = format!("mkdir -p {}", into);
    let extract_cmd = match unpack.format.as_str() {
        "tar" => format!("tar xf {} -C {}", dst, into),
        "tar.gz" | "tgz" => format!("tar xzf {} -C {}", dst, into),
        "zip" => format!("unzip -o {} -d {}", dst, into),
        fmt => {
            return Err(VastError::ParseError(format!(
                "unsupported unpack format: {}",
                fmt
            )))
        }
    };

    Ok(vec![mkdir_cmd, extract_cmd])
}

pub(crate) async fn run_rsync(h: &InstanceHandle, source: &DataSource) -> Result<(), VastError> {
    let ssh_host = h.ssh_host.as_deref().unwrap_or("");
    let ssh_port = h.ssh_port.unwrap_or(22);
    let remote_dst = format!("root@{}:{}", ssh_host, source.dst);
    let ssh_opt = format!("ssh -p {} -o StrictHostKeyChecking=no", ssh_port);

    let status = tokio::process::Command::new("rsync")
        .args([
            "-avz",
            "--partial",
            "-e",
            &ssh_opt,
            &source.src,
            &remote_dst,
        ])
        .status()
        .await?;

    if !status.success() {
        return Err(VastError::CliFailure {
            exit_code: status.code().unwrap_or(-1),
            stderr: "rsync exited with non-zero status".to_string(),
        });
    }
    Ok(())
}

fn ssh_endpoint(h: &InstanceHandle) -> Result<(&str, u16), VastError> {
    let host = h.ssh_host.as_deref().ok_or_else(|| {
        VastError::ParseError(format!("instance {} has no ssh_host", h.id))
    })?;
    let port = h
        .ssh_port
        .ok_or_else(|| VastError::ParseError(format!("instance {} has no ssh_port", h.id)))?;
    if host.is_empty() {
        return Err(VastError::ParseError(format!(
            "instance {} ssh_host is empty",
            h.id
        )));
    }
    Ok((host, port))
}

/// Upload `src` (local file or directory) to `dst` (remote absolute path) via
/// a single tar-pipe over SSH. For directories: contents land *under* `dst`,
/// matching what users expect from `cp -r src/. dst/`. For single files:
/// `dst` becomes the file path. Replaces `vastai cp`, which silently no-ops on
/// directories and was the cause of the "upload ok but instance is empty"
/// blocker (issue.md §2).
async fn tar_upload(h: &InstanceHandle, src: &str, dst: &str) -> Result<(), VastError> {
    let (host, port) = ssh_endpoint(h)?;

    let src_path = Path::new(src);
    let metadata = std::fs::metadata(src_path).map_err(|e| {
        VastError::ParseError(format!("local source {} unreadable: {}", src, e))
    })?;

    let port_str = port.to_string();
    let host_arg = format!("root@{}", host);
    let ssh_args: Vec<String> = vec![
        "-p".to_string(),
        port_str,
        "-o".to_string(),
        "StrictHostKeyChecking=no".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=30".to_string(),
        host_arg,
    ];

    let dst_q = shell_quote(dst);
    let (tar_args, remote_cmd): (Vec<String>, String) = if metadata.is_dir() {
        // tar -cf - -C <src> .
        // remote: mkdir -p <dst> && tar -xf - -C <dst>
        let args = vec![
            "-cf".to_string(),
            "-".to_string(),
            "-C".to_string(),
            src.to_string(),
            ".".to_string(),
        ];
        let cmd = format!("mkdir -p {dst_q} && tar -xf - -C {dst_q}");
        (args, cmd)
    } else {
        // Single file: archive it under its basename, extract into dirname(dst),
        // then rename in place if basename(src) != basename(dst).
        let parent = src_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let basename = src_path
            .file_name()
            .ok_or_else(|| {
                VastError::ParseError(format!("source path has no file name: {}", src))
            })?
            .to_string_lossy()
            .to_string();

        let dst_parent = Path::new(dst)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let dst_basename = Path::new(dst)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| basename.clone());

        let dst_parent_q = shell_quote(&dst_parent);
        let basename_q = shell_quote(&basename);
        let dst_basename_q = shell_quote(&dst_basename);

        let cmd = if basename == dst_basename {
            format!("mkdir -p {dst_parent_q} && tar -xf - -C {dst_parent_q}")
        } else {
            format!(
                "mkdir -p {dst_parent_q} && tar -xf - -C {dst_parent_q} && \
                 mv {dst_parent_q}/{basename_q} {dst_parent_q}/{dst_basename_q}"
            )
        };
        let args = vec![
            "-cf".to_string(),
            "-".to_string(),
            "-C".to_string(),
            parent,
            basename,
        ];
        (args, cmd)
    };

    // Spawn `tar` locally with stdout piped, then `ssh` with that pipe as stdin.
    let mut tar_child = tokio::process::Command::new("tar")
        .args(&tar_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => VastError::CliFailure {
                exit_code: 127,
                stderr: "tar binary not found in PATH (Win10+: built-in at \
                         C:\\Windows\\System32\\tar.exe; Git Bash: included)"
                    .to_string(),
            },
            _ => VastError::Io(e),
        })?;

    let tar_stdout = tar_child
        .stdout
        .take()
        .ok_or_else(|| VastError::ParseError("could not capture tar stdout".to_string()))?;
    let stdin_for_ssh: Stdio = tar_stdout.try_into().map_err(VastError::Io)?;

    let ssh_status = tokio::process::Command::new("ssh")
        .args(&ssh_args)
        .arg(&remote_cmd)
        .stdin(stdin_for_ssh)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => VastError::CliFailure {
                exit_code: 127,
                stderr: "ssh binary not found in PATH (Win10+: optional feature \
                         OpenSSH.Client, or use Git for Windows)"
                    .to_string(),
            },
            _ => VastError::Io(e),
        })?
        .wait_with_output()
        .await
        .map_err(VastError::Io)?;

    let tar_status = tar_child.wait().await.map_err(VastError::Io)?;

    // When ssh dies mid-stream (sshd not ready, network drop, dst FS full),
    // tar gets EPIPE and reports its own non-zero exit. The tar exit is then
    // the *symptom*, not the root cause — the user needs to see ssh's stderr
    // to diagnose. So: report ssh first, even if tar also failed.
    if !ssh_status.status.success() {
        let stderr = String::from_utf8_lossy(&ssh_status.stderr).trim().to_string();
        return Err(VastError::CliFailure {
            exit_code: ssh_status.status.code().unwrap_or(-1),
            stderr: format!(
                "ssh tar-extract at {} failed (exit {:?}): {}",
                dst,
                ssh_status.status.code(),
                if stderr.is_empty() {
                    "(no stderr — usually means ssh was killed before the remote command produced output; \
                     try again or check sshd readiness)"
                        .to_string()
                } else {
                    stderr
                }
            ),
        });
    }
    if !tar_status.success() {
        return Err(VastError::CliFailure {
            exit_code: tar_status.code().unwrap_or(-1),
            stderr: format!("local tar failed for {}: exit {:?}", src, tar_status.code()),
        });
    }
    Ok(())
}

/// Sanity-check that the upload actually delivered bytes. Hits the remote with
/// `du -sb <dst>` and refuses to advance if the destination is missing or 0
/// bytes. Catches the silent-no-op class of bug that motivated this
/// reimplementation (issue.md §2).
async fn verify_upload(h: &InstanceHandle, dst: &str) -> Result<u64, VastError> {
    let (host, port) = ssh_endpoint(h)?;
    let dst_q = shell_quote(dst);
    let cmd = format!(
        "if [ ! -e {dst_q} ]; then echo MISSING; exit 0; fi; \
         du -sb {dst_q} 2>/dev/null | awk '{{print $1}}'"
    );
    let raw = crate::transfer::ssh_exec(host, port, &cmd).await?;
    let out = String::from_utf8_lossy(&raw).trim().to_string();

    if out == "MISSING" {
        return Err(VastError::CliFailure {
            exit_code: 0,
            stderr: format!(
                "upload verification: {} does not exist on the instance — the upload \
                 silently no-op'd. (vastai cp directory bug, or local source path was wrong.)",
                dst
            ),
        });
    }
    let bytes: u64 = out.parse().map_err(|_| VastError::ParseError(format!(
        "upload verification: unexpected `du -sb` output for {}: {:?}",
        dst, out
    )))?;
    if bytes == 0 {
        return Err(VastError::CliFailure {
            exit_code: 0,
            stderr: format!(
                "upload verification: {} is empty (0 bytes) — refusing to advance to \
                 train_start. Check the local source path and the upload mode.",
                dst
            ),
        });
    }
    tracing::info!("upload: {} → {} bytes on instance", dst, bytes);
    Ok(bytes)
}

/// Upload all data sources to the remote instance.
/// Dispatches each source to copy, rsync, or unpack logic based on its mode/unpack fields.
pub(crate) async fn upload_sources(
    instance_id: InstanceId,
    h: &InstanceHandle,
    sources: &[DataSource],
) -> Result<(), VastError> {
    // Vast.ai reports actual_status=running well before sshd inside the
    // container is reachable — TCP layer up, but `ssh root@…` either rejects
    // or stalls. Without this gate `tar | ssh` dies on EPIPE in seconds and
    // 4 GB of data never reaches the disk. Wait up to 2 min, polling every
    // 3 s; this matches the typical 30–60 s warm-up we see in practice.
    if !sources.is_empty() {
        if let (Some(host), Some(port)) = (h.ssh_host.as_deref(), h.ssh_port) {
            let waited = crate::transfer::wait_for_ssh_ready(
                host,
                port,
                std::time::Duration::from_secs(3),
                std::time::Duration::from_secs(120),
            )
            .await?;
            tracing::info!(
                "ssh ready on {}:{} after {:.1}s — starting upload",
                host,
                port,
                waited.as_secs_f64()
            );
        }
    }

    for source in sources {
        match source.mode.as_ref() {
            None | Some(DataMode::Copy) => {
                tar_upload(h, &source.src, &source.dst).await?;
            }
            Some(DataMode::Rsync) => {
                which::which("rsync").map_err(|_| VastError::RsyncNotFound)?;
                run_rsync(h, source).await?;
            }
        }

        verify_upload(h, &source.dst).await?;

        for cmd in unpack_commands(source)? {
            crate::cli::execute(instance_id, &cmd).await?;
        }
    }
    Ok(())
}
