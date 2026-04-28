#![deny(unsafe_code)]

use std::path::PathBuf;

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

/// Upload all data sources to the remote instance.
/// Dispatches each source to copy, rsync, or unpack logic based on its mode/unpack fields.
pub(crate) async fn upload_sources(
    instance_id: InstanceId,
    h: &InstanceHandle,
    sources: &[DataSource],
) -> Result<(), VastError> {
    for source in sources {
        match source.mode.as_ref() {
            None | Some(DataMode::Copy) => {
                let (src, dst) = copy_endpoints(instance_id, source);
                crate::cli::copy(&src, &dst).await?;
            }
            Some(DataMode::Rsync) => {
                which::which("rsync").map_err(|_| VastError::RsyncNotFound)?;
                run_rsync(h, source).await?;
            }
        }

        for cmd in unpack_commands(source)? {
            crate::cli::execute(instance_id, &cmd).await?;
        }
    }
    Ok(())
}
