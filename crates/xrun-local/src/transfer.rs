#![deny(unsafe_code)]

//! Local file transfer helpers — `fs::copy` for files, recursive copy for
//! directories, glob-based discovery for `pull`. Stage 1 (Phase 0) does not
//! support `rsync`, `unpack`, `exclude` or `compress`; the adapter warns and
//! falls back to a plain copy.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::LocalError;

/// One copied file plus the metadata needed to record it as an artifact.
#[derive(Debug)]
pub struct CopiedArtifact {
    pub remote_path: String,
    pub local_path: PathBuf,
    pub size_bytes: i64,
    pub sha256: String,
}

/// Copy `src` to `dst`. Files are copied directly; directories are walked
/// recursively. Parent directories are created as needed.
pub fn copy_path(src: &Path, dst: &Path) -> Result<u64, LocalError> {
    let meta = std::fs::metadata(src)
        .map_err(|e| LocalError::Spawn(format!("data src missing or unreadable {src:?}: {e}")))?;
    if meta.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::copy(src, dst)?)
    } else if meta.is_dir() {
        copy_dir_recursive(src, dst)
    } else {
        Err(LocalError::Spawn(format!(
            "unsupported source type at {src:?}"
        )))
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<u64, LocalError> {
    std::fs::create_dir_all(dst)?;
    let mut total: u64 = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let m = entry.metadata()?;
        if m.is_dir() {
            total = total.saturating_add(copy_dir_recursive(&from, &to)?);
        } else if m.is_file() {
            total = total.saturating_add(std::fs::copy(&from, &to)?);
        }
        // Symlinks intentionally skipped — they would copy a dangling link
        // most of the time and the local data: spec doesn't promise to
        // resolve them.
    }
    Ok(total)
}

/// Resolve `pattern` against `workdir`. The pattern may be:
/// - absolute (`/abs/...`, `C:\...`)
/// - workdir-relative (`checkpoints/best*.pt`)
///
/// Returns matched files (directories filtered out so `record_artifact` rows
/// always represent a real file).
pub fn glob_in_workdir(workdir: &Path, pattern: &str) -> Result<Vec<PathBuf>, LocalError> {
    let p = Path::new(pattern);
    let abs_pattern = if p.is_absolute() {
        pattern.to_string()
    } else {
        workdir.join(pattern).display().to_string()
    };
    let entries = glob::glob(&abs_pattern)
        .map_err(|e| LocalError::Spawn(format!("invalid glob pattern {pattern:?}: {e}")))?;
    let mut out = Vec::new();
    for path in entries.flatten() {
        if path.is_file() {
            out.push(path);
        }
    }
    Ok(out)
}

/// Copy `matches` into `into_dir` and produce per-file metadata for the
/// artifact store. `remote_path` is the file's path relative to `workdir`
/// (or absolute when the source lay outside the workdir tree).
pub fn pull_matches(
    matches: &[PathBuf],
    workdir: &Path,
    into_dir: &Path,
) -> Result<Vec<CopiedArtifact>, LocalError> {
    std::fs::create_dir_all(into_dir)?;
    let mut out = Vec::with_capacity(matches.len());
    for src in matches {
        let file_name = src
            .file_name()
            .ok_or_else(|| LocalError::Spawn(format!("source has no filename: {src:?}")))?;
        let dst = into_dir.join(file_name);
        std::fs::copy(src, &dst)?;
        let bytes = std::fs::metadata(&dst)?.len() as i64;
        let sha = sha256_of(&dst)?;
        let remote_path = src
            .strip_prefix(workdir)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| src.display().to_string());
        out.push(CopiedArtifact {
            remote_path,
            local_path: dst,
            size_bytes: bytes,
            sha256: sha,
        });
    }
    Ok(out)
}

fn sha256_of(path: &Path) -> Result<String, LocalError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        use std::io::Read;
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Classify an artifact kind from a filename's extension. Mirrors the vast
/// adapter so `xrun show <id>` renders the same kinds across vendors.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn copy_single_file() {
        let td = TempDir::new().unwrap();
        let src = td.path().join("a.txt");
        std::fs::write(&src, b"hello").unwrap();
        let dst = td.path().join("nested/copy.txt");
        let bytes = copy_path(&src, &dst).unwrap();
        assert_eq!(bytes, 5);
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello");
    }

    #[test]
    fn copy_directory_recursively() {
        let td = TempDir::new().unwrap();
        let src = td.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::write(src.join("sub/b.txt"), b"bb").unwrap();
        let dst = td.path().join("dst");
        let total = copy_path(&src, &dst).unwrap();
        assert_eq!(total, 3);
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.join("sub/b.txt")).unwrap(), b"bb");
    }

    #[test]
    fn glob_resolves_relative_to_workdir() {
        let td = TempDir::new().unwrap();
        let work = td.path().join("work");
        std::fs::create_dir_all(work.join("ckpt")).unwrap();
        std::fs::write(work.join("ckpt/best_00.pt"), b"a").unwrap();
        std::fs::write(work.join("ckpt/best_01.pt"), b"b").unwrap();
        std::fs::write(work.join("ckpt/notes.txt"), b"c").unwrap();
        let matches = glob_in_workdir(&work, "ckpt/best_*.pt").unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn glob_filters_out_directories() {
        let td = TempDir::new().unwrap();
        let work = td.path().join("work");
        std::fs::create_dir_all(work.join("a/b")).unwrap();
        let matches = glob_in_workdir(&work, "a/*").unwrap();
        assert!(matches.is_empty(), "got: {matches:?}");
    }

    #[test]
    fn pull_matches_records_size_and_hash() {
        let td = TempDir::new().unwrap();
        let work = td.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let src = work.join("model.pt");
        std::fs::write(&src, b"weights").unwrap();
        let into = td.path().join("models");
        let pulled = pull_matches(std::slice::from_ref(&src), &work, &into).unwrap();
        assert_eq!(pulled.len(), 1);
        assert_eq!(pulled[0].size_bytes, 7);
        assert_eq!(pulled[0].sha256.len(), 64);
        assert_eq!(pulled[0].remote_path, "model.pt");
    }

    #[test]
    fn classify_known_extensions() {
        assert_eq!(classify_kind("best.ckpt"), "checkpoint");
        assert_eq!(classify_kind("loss.png"), "figure");
        assert_eq!(classify_kind("metrics.json"), "json");
        assert_eq!(classify_kind("train.log"), "log");
        assert_eq!(classify_kind("README"), "other");
    }
}
