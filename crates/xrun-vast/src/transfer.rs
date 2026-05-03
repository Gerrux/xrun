#![deny(unsafe_code)]

//! Instance-to-instance file transfer via tar piped through local machine.
//!
//! Data streams directly from source SSH stdout to destination SSH stdin —
//! no intermediate file is written locally, so even large transfers (tens of
//! GB) don't require free disk space on the orchestrating machine.

use std::process::Stdio;

use crate::error::VastError;

pub struct SshConn {
    pub host: String,
    pub port: u16,
}

pub enum TransferEndpoint {
    Local(String),
    Remote(SshConn, String),
}

/// Resolve the SSH connection details for a vast.ai instance by ID.
pub async fn resolve_ssh(instance_id: u64, api_key: &str) -> Result<SshConn, VastError> {
    let instances = crate::rest::show_instances(api_key).await?;
    let inst = instances
        .into_iter()
        .find(|i| i.id == instance_id)
        .ok_or_else(|| VastError::ParseError(format!("instance {instance_id} not found")))?;
    let host = inst.ssh_host.ok_or_else(|| {
        VastError::ParseError(format!(
            "instance {instance_id} has no ssh_host (not running?)"
        ))
    })?;
    let port = inst.ssh_port.ok_or_else(|| {
        VastError::ParseError(format!(
            "instance {instance_id} has no ssh_port (not running?)"
        ))
    })?;
    if host.is_empty() {
        return Err(VastError::ParseError(format!(
            "instance {instance_id} ssh_host is empty"
        )));
    }
    Ok(SshConn { host, port })
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Wait until the instance's sshd accepts a real command. Combines a TCP
/// reachability probe with a `ssh … true` round-trip — TCP-connect alone is
/// not sufficient because vast.ai's TCP layer (proxy) is up well before sshd
/// is ready inside the container, and a premature `tar | ssh` then dies with
/// `Connection reset by peer` while tar is still streaming its first MB.
///
/// Retries every `interval` until either success or `timeout`. Both must be
/// non-zero. Returns the elapsed wait so callers can log it.
pub async fn wait_for_ssh_ready(
    host: &str,
    port: u16,
    interval: std::time::Duration,
    timeout: std::time::Duration,
) -> Result<std::time::Duration, VastError> {
    let start = std::time::Instant::now();
    let mut last_err: Option<String> = None;
    let addr = format!("{}:{}", host, port);

    while start.elapsed() < timeout {
        // Stage 1: TCP-level reachability — fast fail if the proxy/router
        // hasn't published the port yet.
        let tcp_ok = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .is_some();

        if tcp_ok {
            // Stage 2: real sshd handshake. Use a tiny no-op command so we
            // exercise the full auth + exec path. `ConnectTimeout=5` here is
            // intentional — under load sshd can answer the TCP SYN but stall
            // on key exchange.
            let host_arg = format!("root@{}", host);
            let port_str = port.to_string();
            let mut probe_cmd = tokio::process::Command::new("ssh");
            probe_cmd.args([
                "-p",
                &port_str,
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                &host_arg,
                "true",
            ]);
            #[cfg(windows)]
            {
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                probe_cmd.creation_flags(CREATE_NO_WINDOW);
            }
            let probe = probe_cmd.output().await;
            match probe {
                Ok(out) if out.status.success() => return Ok(start.elapsed()),
                Ok(out) => {
                    last_err = Some(format!(
                        "ssh probe exit {:?}: {}",
                        out.status.code(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                Err(e) => last_err = Some(format!("ssh probe spawn: {}", e)),
            }
        } else {
            last_err = Some(format!("tcp connect {} failed", addr));
        }
        tokio::time::sleep(interval).await;
    }

    Err(VastError::CliFailure {
        exit_code: 0,
        stderr: format!(
            "ssh not ready on {}:{} after {}s — last error: {}",
            host,
            port,
            timeout.as_secs(),
            last_err.unwrap_or_else(|| "(no probe ran)".to_string())
        ),
    })
}

/// Run an arbitrary shell command on a remote via SSH and return its stdout.
/// Bypasses vast.ai's `instances/<id>/execute` API, which rejects compound
/// shell forms (`nohup … &`, pipes, here-docs, multi-line). Plain SSH accepts
/// anything the remote shell does.
pub async fn ssh_exec(host: &str, port: u16, cmd: &str) -> Result<Vec<u8>, VastError> {
    let host_arg = format!("root@{}", host);
    let port_str = port.to_string();
    let mut ssh_exec_cmd = tokio::process::Command::new("ssh");
    ssh_exec_cmd.args([
        "-p",
        &port_str,
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=30",
        &host_arg,
        cmd,
    ]);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        ssh_exec_cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = ssh_exec_cmd.output().await.map_err(VastError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        return Err(VastError::CliFailure {
            exit_code: output.status.code().unwrap_or(-1),
            stderr: format!("ssh root@{}:{} → {}", host, port, msg),
        });
    }
    Ok(output.stdout)
}

/// Copy a single file from a remote vast.ai instance to a local path via scp.
/// Replaces `vastai copy <id>:<remote> <local>` with a direct call to scp,
/// dropping the dependency on the vastai Python CLI.
pub async fn scp_pull(
    host: &str,
    port: u16,
    remote_path: &str,
    local_path: &std::path::Path,
) -> Result<(), VastError> {
    let port_str = port.to_string();
    let remote_arg = format!("root@{}:{}", host, remote_path);
    let local_arg = local_path.to_string_lossy().to_string();
    let mut scp = tokio::process::Command::new("scp");
    scp.args([
        "-P",
        &port_str,
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=30",
        &remote_arg,
        &local_arg,
    ]);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        scp.creation_flags(CREATE_NO_WINDOW);
    }
    let output = scp.output().await.map_err(VastError::Io)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(VastError::CliFailure {
            exit_code: output.status.code().unwrap_or(-1),
            stderr: format!("scp {} → {}: {}", remote_arg, local_arg, stderr),
        });
    }
    Ok(())
}

fn ssh_cmd(conn: &SshConn) -> std::process::Command {
    let mut cmd = std::process::Command::new("ssh");
    cmd.args([
        "-p",
        &conn.port.to_string(),
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=30",
        &format!("root@{}", conn.host),
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Transfer files from `src` to `dst` using a tar pipe.
///
/// For Remote→Remote: pipes `ssh src "tar cf - PATH"` directly into
/// `ssh dst "tar xf - -C DST"` — no local disk I/O beyond the pipe buffer.
///
/// `verbose`: pass `-v` to tar so filenames are printed to stderr.
pub fn transfer(
    src: &TransferEndpoint,
    dst: &TransferEndpoint,
    verbose: bool,
) -> Result<(), VastError> {
    let tar_c_flag = if verbose { "-cvf" } else { "-cf" };
    let tar_x_flag = if verbose { "-xvf" } else { "-xf" };

    let mut src_child: std::process::Child = match src {
        TransferEndpoint::Remote(conn, path) => {
            let remote_cmd = format!(
                "tar --one-file-system {} - -- {}",
                tar_c_flag,
                shell_quote(path)
            );
            ssh_cmd(conn)
                .arg(&remote_cmd)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| VastError::ParseError(format!("spawn ssh src: {e}")))?
        }
        TransferEndpoint::Local(path) => {
            let p = std::path::Path::new(path);
            let parent = p.parent().unwrap_or(std::path::Path::new("."));
            let name = p.file_name().unwrap_or_default();
            let mut tar_src = std::process::Command::new("tar");
            tar_src
                .arg("-C")
                .arg(parent)
                .arg(tar_c_flag)
                .arg("-")
                .arg("--")
                .arg(name)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                tar_src.creation_flags(CREATE_NO_WINDOW);
            }
            tar_src
                .spawn()
                .map_err(|e| VastError::ParseError(format!("spawn local tar src: {e}")))?
        }
    };

    let src_out = src_child
        .stdout
        .take()
        .ok_or_else(|| VastError::ParseError("no stdout pipe from src".into()))?;

    let mut dst_child: std::process::Child = match dst {
        TransferEndpoint::Remote(conn, path) => {
            let remote_cmd = format!(
                "mkdir -p {} && tar {} - -C {}",
                shell_quote(path),
                tar_x_flag,
                shell_quote(path)
            );
            ssh_cmd(conn)
                .arg(&remote_cmd)
                .stdin(Stdio::from(src_out))
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| VastError::ParseError(format!("spawn ssh dst: {e}")))?
        }
        TransferEndpoint::Local(path) => {
            std::fs::create_dir_all(path)
                .map_err(|e| VastError::ParseError(format!("create dst dir: {e}")))?;
            let mut tar_dst = std::process::Command::new("tar");
            tar_dst
                .arg(tar_x_flag)
                .arg("-")
                .arg("-C")
                .arg(path)
                .stdin(Stdio::from(src_out))
                .stderr(Stdio::inherit());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                tar_dst.creation_flags(CREATE_NO_WINDOW);
            }
            tar_dst
                .spawn()
                .map_err(|e| VastError::ParseError(format!("spawn local tar dst: {e}")))?
        }
    };

    // Wait for destination first (it drains the pipe), then source.
    let dst_status = dst_child
        .wait()
        .map_err(|e| VastError::ParseError(format!("wait dst: {e}")))?;
    let src_status = src_child
        .wait()
        .map_err(|e| VastError::ParseError(format!("wait src: {e}")))?;

    if !src_status.success() {
        return Err(VastError::CliFailure {
            exit_code: src_status.code().unwrap_or(-1),
            stderr: "source tar/ssh exited with non-zero status".into(),
        });
    }
    if !dst_status.success() {
        return Err(VastError::CliFailure {
            exit_code: dst_status.code().unwrap_or(-1),
            stderr: "destination tar/ssh exited with non-zero status".into(),
        });
    }

    Ok(())
}
