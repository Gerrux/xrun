#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use xrun_core::{
    manifest::{DataCompress, DataMode, DataSource},
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

    let mut rsync_cmd = tokio::process::Command::new("rsync");
    rsync_cmd.args([
        "-avz",
        "--partial",
        "-e",
        &ssh_opt,
        &source.src,
        &remote_dst,
    ]);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        rsync_cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let status = rsync_cmd.status().await?;

    if !status.success() {
        return Err(VastError::CliFailure {
            exit_code: status.code().unwrap_or(-1),
            stderr: "rsync exited with non-zero status".to_string(),
        });
    }
    Ok(())
}

fn ssh_endpoint(h: &InstanceHandle) -> Result<(&str, u16), VastError> {
    let host = h
        .ssh_host
        .as_deref()
        .ok_or_else(|| VastError::ParseError(format!("instance {} has no ssh_host", h.id)))?;
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
async fn tar_upload(
    h: &InstanceHandle,
    source: &DataSource,
    timeout: Option<std::time::Duration>,
) -> Result<(), VastError> {
    let src = source.src.as_str();
    let dst = source.dst.as_str();
    let exclude = source.exclude.as_slice();
    let compress = source.compress.unwrap_or(DataCompress::None);
    let (host, port) = ssh_endpoint(h)?;

    let src_path = Path::new(src);
    let metadata = std::fs::metadata(src_path)
        .map_err(|e| VastError::ParseError(format!("local source {} unreadable: {}", src, e)))?;

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

    // Compression: local tar gets a compress flag; remote tar gets a matching
    // decompress flag. For zstd we pre-check the remote binary exists before
    // starting the multi-GB pipe — missing zstd gives a clear error instead of
    // an opaque mid-stream exit code. For gzip we emit a pigz hint in the
    // remote command so `apt-get install -y pigz` in setup immediately unlocks
    // parallel decompression without any xrun changes.
    if compress == DataCompress::Zstd {
        // Check that zstd is available on the remote before committing to the
        // pipe. One SSH round-trip here saves a confusing mid-stream failure.
        let check = crate::transfer::ssh_exec(
            host,
            port,
            "command -v zstd >/dev/null 2>&1 || { echo 'zstd not found'; exit 127; }",
        )
        .await;
        if check.is_err() {
            return Err(VastError::CliFailure {
                exit_code: 127,
                stderr: format!(
                    "zstd binary not found on the remote instance ({}:{}). \
                     Either set `compress: gzip` in the manifest, or add \
                     `apt-get install -y zstd` to `run.setup`.",
                    host, port
                ),
            });
        }
    }

    if compress == DataCompress::Gzip {
        // Single-threaded gzip caps decompression at ~10% of one core, which
        // turns a 14-minute upload+extract into a 2-hour one on multi-GB
        // datasets. pigz is a drop-in parallel replacement; the remote tar
        // command already prefers it when present. Install once per
        // instance, before the upload starts; idempotent via `command -v`.
        // Quiet on failure so a base image without apt (e.g. alpine) still
        // succeeds — gzip just stays single-threaded in that case.
        let _ = crate::transfer::ssh_exec(
            host,
            port,
            "command -v pigz >/dev/null 2>&1 || \
             (export DEBIAN_FRONTEND=noninteractive; \
              apt-get update -qq && apt-get install -y -qq pigz) \
             >/dev/null 2>&1 || true",
        )
        .await;
    }

    let (compress_flag, remote_compress_flag): (Option<&str>, Option<&str>) = match compress {
        DataCompress::None => (None, None),
        // Remote side: prefer pigz (parallel gzip) when installed; fall back
        // to the standard -z flag. The $() substitution is evaluated by the
        // remote shell — it's safe because we pass the command as a single SSH
        // argument, not through a local shell.
        DataCompress::Gzip => (
            Some("-z"),
            Some("--use-compress-program=\"$(command -v pigz 2>/dev/null || echo gzip)\""),
        ),
        DataCompress::Zstd => (Some("--zstd"), Some("--zstd")),
    };
    let mut excl_args: Vec<String> = Vec::with_capacity(exclude.len());
    for pat in exclude {
        excl_args.push(format!("--exclude={pat}"));
    }

    let dst_q = shell_quote(dst);
    let (tar_args, remote_cmd): (Vec<String>, String) = if metadata.is_dir() {
        // tar [--zstd|-z] -cf - -C <src> [--exclude=...] .
        // remote: mkdir -p <dst> && tar [...] -xf - -C <dst>
        let mut args: Vec<String> = Vec::new();
        if let Some(f) = compress_flag {
            args.push(f.to_string());
        }
        args.extend_from_slice(&[
            "-cf".to_string(),
            "-".to_string(),
            "-C".to_string(),
            src.to_string(),
        ]);
        args.extend(excl_args.iter().cloned());
        args.push(".".to_string());

        let rcompress = remote_compress_flag
            .map(|f| format!("{f} "))
            .unwrap_or_default();
        let cmd = format!("mkdir -p {dst_q} && tar {rcompress}-xf - -C {dst_q}");
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
            .ok_or_else(|| VastError::ParseError(format!("source path has no file name: {}", src)))?
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

        let rcompress = remote_compress_flag
            .map(|f| format!("{f} "))
            .unwrap_or_default();
        let cmd = if basename == dst_basename {
            format!("mkdir -p {dst_parent_q} && tar {rcompress}-xf - -C {dst_parent_q}")
        } else {
            format!(
                "mkdir -p {dst_parent_q} && tar {rcompress}-xf - -C {dst_parent_q} && \
                 mv {dst_parent_q}/{basename_q} {dst_parent_q}/{dst_basename_q}"
            )
        };
        let mut args: Vec<String> = Vec::new();
        if let Some(f) = compress_flag {
            args.push(f.to_string());
        }
        args.extend_from_slice(&["-cf".to_string(), "-".to_string(), "-C".to_string(), parent]);
        args.extend(excl_args.iter().cloned());
        args.push(basename);
        (args, cmd)
    };

    // Spawn `tar` locally with stdout piped, then `ssh` with that pipe as stdin.
    // Both children get `kill_on_drop(true)` so a cancelled future (e.g.
    // `tokio::time::timeout`) terminates the local processes — without this
    // the user is left with orphan tar.exe/ssh.exe writing into a closed pipe.
    let mut tar_cmd = tokio::process::Command::new("tar");
    tar_cmd
        .args(&tar_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        tar_cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut tar_child = tar_cmd.spawn().map_err(|e| match e.kind() {
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

    let mut ssh_cmd = tokio::process::Command::new("ssh");
    ssh_cmd
        .args(&ssh_args)
        .arg(&remote_cmd)
        .stdin(stdin_for_ssh)
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        ssh_cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let ssh_child = ssh_cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => VastError::CliFailure {
            exit_code: 127,
            stderr: "ssh binary not found in PATH (Win10+: optional feature \
                         OpenSSH.Client, or use Git for Windows)"
                .to_string(),
        },
        _ => VastError::Io(e),
    })?;

    // Run upload with optional deadline. On timeout: drop both children
    // (kill_on_drop fires), probe the destination one last time so the error
    // can report progress + effective throughput.
    let started = std::time::Instant::now();
    let host_owned = host.to_string();
    let dst_owned = dst.to_string();
    let upload_fut = async {
        let ssh_out = ssh_child.wait_with_output().await.map_err(VastError::Io)?;
        let tar_out = tar_child.wait().await.map_err(VastError::Io)?;
        Ok::<_, VastError>((ssh_out, tar_out))
    };

    let (ssh_status, tar_status) = if let Some(timeout) = timeout {
        match tokio::time::timeout(timeout, upload_fut).await {
            Ok(Ok((ssh_out, tar_out))) => (ssh_out, tar_out),
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                // The two child handles have been dropped by the cancelled
                // future; kill_on_drop will reap them. Best-effort probe to
                // tell the user how much actually transferred.
                let elapsed = started.elapsed().as_secs();
                let transferred = du_bytes(&host_owned, port, &dst_owned).await.unwrap_or(0);
                let mbps = if elapsed > 0 {
                    (transferred as f64 * 8.0 / 1_000_000.0) / elapsed as f64
                } else {
                    0.0
                };
                return Err(VastError::UploadTimeout {
                    dst: dst_owned,
                    transferred,
                    elapsed_secs: elapsed,
                    mbps,
                });
            }
        }
    } else {
        match upload_fut.await {
            Ok(pair) => pair,
            Err(e) => return Err(e),
        }
    };

    // When ssh dies mid-stream (sshd not ready, network drop, dst FS full),
    // tar gets EPIPE and reports its own non-zero exit. The tar exit is then
    // the *symptom*, not the root cause — the user needs to see ssh's stderr
    // to diagnose. So: report ssh first, even if tar also failed.
    if !ssh_status.status.success() {
        let stderr = String::from_utf8_lossy(&ssh_status.stderr)
            .trim()
            .to_string();
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

/// Probe `du -sb <dst>` on the remote and return its size in bytes. Returns
/// 0 when the path doesn't exist yet (e.g. before tar has started writing).
/// Used both for live progress reporting and for the post-mortem "how far did
/// we get" line in `UploadTimeout`.
async fn du_bytes(host: &str, port: u16, dst: &str) -> Result<u64, VastError> {
    let dst_q = shell_quote(dst);
    let cmd = format!(
        "if [ ! -e {dst_q} ]; then echo 0; else du -sb {dst_q} 2>/dev/null | awk '{{print $1}}'; fi"
    );
    let raw = crate::transfer::ssh_exec(host, port, &cmd).await?;
    let out = String::from_utf8_lossy(&raw).trim().to_string();
    Ok(out.parse().unwrap_or(0))
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
    let bytes: u64 = out.parse().map_err(|_| {
        VastError::ParseError(format!(
            "upload verification: unexpected `du -sb` output for {}: {:?}",
            dst, out
        ))
    })?;
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
///
/// `timeout` is applied *per source*. A 4 KB launch script and a 4 GB dataset
/// each get their own budget — sharing one global deadline meant the script
/// always sailed through and the dataset always blew up.
pub(crate) async fn upload_sources(
    instance_id: InstanceId,
    h: &InstanceHandle,
    sources: &[DataSource],
    timeout: Option<std::time::Duration>,
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
        // Live progress probe: every 30s log measured destination size and
        // effective Mbps. Tracing-only for now; the daemon doesn't have a
        // back-channel from upload.rs to the events table.
        let progress_handle = if let (Some(host), Some(port)) =
            (h.ssh_host.as_deref().map(|s| s.to_string()), h.ssh_port)
        {
            let dst = source.dst.clone();
            let started = std::time::Instant::now();
            Some(tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
                tick.tick().await;
                loop {
                    tick.tick().await;
                    let bytes = du_bytes(&host, port, &dst).await.unwrap_or(0);
                    let elapsed = started.elapsed().as_secs().max(1);
                    let mbps = (bytes as f64 * 8.0 / 1_000_000.0) / elapsed as f64;
                    tracing::info!(
                        "upload progress: {} = {} bytes after {}s ({:.1} Mbps)",
                        dst,
                        bytes,
                        elapsed,
                        mbps
                    );
                }
            }))
        } else {
            None
        };

        let result = match source.mode.as_ref() {
            None | Some(DataMode::Copy) => tar_upload(h, source, timeout).await,
            Some(DataMode::Rsync) => {
                which::which("rsync").map_err(|_| VastError::RsyncNotFound)?;
                run_rsync(h, source).await
            }
        };

        if let Some(handle) = progress_handle {
            handle.abort();
        }
        result?;

        verify_upload(h, &source.dst).await?;

        for cmd in unpack_commands(source)? {
            let host = h.ssh_host.as_deref().ok_or_else(|| {
                VastError::ParseError(format!(
                    "instance {instance_id} has no ssh_host for unpack"
                ))
            })?;
            let port = h.ssh_port.ok_or_else(|| {
                VastError::ParseError(format!(
                    "instance {instance_id} has no ssh_port for unpack"
                ))
            })?;
            crate::transfer::ssh_exec(host, port, &cmd).await?;
        }
    }
    Ok(())
}
