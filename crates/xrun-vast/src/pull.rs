#![deny(unsafe_code)]

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{
    cli::InstanceId,
    error::VastError,
    transfer::{scp_pull, ssh_exec},
};

/// Classify an artifact kind from the file extension.
pub fn classify_kind(filename: &str) -> String {
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

/// Return true if the string contains glob wildcard characters.
pub fn has_wildcard(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// Parse `ls -1` stdout into a list of trimmed, non-empty path strings.
pub fn parse_ls_output(bytes: &[u8]) -> Vec<String> {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Compute the hex-encoded SHA-256 digest of a local file.
pub fn sha256_of_file(path: &Path) -> Result<String, VastError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Metadata for a successfully pulled artifact file.
pub struct PulledFile {
    pub remote_path: String,
    pub local_path: PathBuf,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
}

/// Pull one or more files from a remote instance into a local directory.
///
/// When `remote_glob` contains wildcards, `ls -1 <glob>` is run first to
/// enumerate matching paths, then each file is copied individually.
pub async fn pull_files(
    host: &str,
    port: u16,
    _instance_id: InstanceId,
    remote_glob: &str,
    into: &Path,
) -> Result<Vec<PulledFile>, VastError> {
    std::fs::create_dir_all(into)?;

    let remote_paths: Vec<String> = if has_wildcard(remote_glob) {
        let ls_cmd = format!("ls -1 {}", remote_glob);
        let ls_out = ssh_exec(host, port, &ls_cmd).await?;
        parse_ls_output(&ls_out)
    } else {
        vec![remote_glob.to_string()]
    };

    let mut pulled = Vec::new();
    for remote_path in remote_paths {
        let filename = Path::new(&remote_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| remote_path.clone());
        let local_path = into.join(&filename);

        scp_pull(host, port, &remote_path, &local_path).await?;

        let size_bytes = local_path.metadata().map(|m| m.len() as i64).ok();
        let sha256 = sha256_of_file(&local_path).ok();

        pulled.push(PulledFile {
            remote_path,
            local_path,
            size_bytes,
            sha256,
        });
    }

    Ok(pulled)
}

/// Delete files beyond the `keep_last` newest entries, sorting by modification
/// time (oldest first). Files that cannot be stat'd sort to the front.
pub fn apply_keep_last(files: &mut Vec<PathBuf>, keep_last: u32) {
    if files.len() <= keep_last as usize {
        return;
    }
    files.sort_by_key(|p| {
        p.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    let to_delete = files.len() - keep_last as usize;
    for path in files.drain(..to_delete) {
        let _ = std::fs::remove_file(&path);
    }
}
