#![deny(unsafe_code)]

//! Local snapshot of a dataset staging directory, used to surface a diff
//! against the previously pushed version. Kaggle's `datasets version` is
//! silent about which files actually moved — when only 3 of 5 files print
//! `Starting upload for file ...` it's not clear whether the other two were
//! identical to the prior version or quietly skipped due to a bug. We solve
//! it locally: fingerprint the staging dir before push, compare against the
//! sidecar from the last push, print the diff, and overwrite on success.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub size: u64,
    /// mtime as seconds since epoch (signed — pre-1970 mtimes are rare but legal).
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub slug: String,
    pub captured_at: String,
    /// Path relative to staging dir → fingerprint.
    pub files: BTreeMap<String, FileEntry>,
}

/// Per-slug diff between current staging dir and the previously pushed snapshot.
#[derive(Debug, Default)]
pub struct SnapshotDiff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub unchanged: Vec<String>,
    pub removed: Vec<String>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "  added:     {}\n",
            if self.added.is_empty() {
                "(none)".to_string()
            } else {
                self.added.join(", ")
            }
        ));
        out.push_str(&format!(
            "  changed:   {}\n",
            if self.changed.is_empty() {
                "(none)".to_string()
            } else {
                self.changed.join(", ")
            }
        ));
        out.push_str(&format!(
            "  removed:   {}\n",
            if self.removed.is_empty() {
                "(none)".to_string()
            } else {
                self.removed.join(", ")
            }
        ));
        out.push_str(&format!("  unchanged: {} files", self.unchanged.len()));
        out
    }
}

/// Walk `local_dir` and capture (path, size, mtime) for every file, ignoring
/// `dataset-metadata.json` (regenerated each push, drifts on its own).
pub fn capture(local_dir: &Path, slug: &str) -> std::io::Result<Snapshot> {
    let mut files = BTreeMap::new();
    walk(local_dir, local_dir, &mut files)?;
    Ok(Snapshot {
        slug: slug.to_string(),
        captured_at: chrono::Utc::now().to_rfc3339(),
        files,
    })
}

fn walk(root: &Path, cur: &Path, out: &mut BTreeMap<String, FileEntry>) -> std::io::Result<()> {
    for entry in fs::read_dir(cur)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(root, &path, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if rel == "dataset-metadata.json" {
                continue;
            }
            let meta = entry.metadata()?;
            let size = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.insert(rel, FileEntry { size, mtime });
        }
    }
    Ok(())
}

/// Where the sidecar lives. Slug `<owner>/<name>` → `<dir>/<owner>__<name>.json`
/// to keep paths flat (and slash-free on Windows).
pub fn sidecar_path(snapshots_dir: &Path, slug: &str) -> PathBuf {
    let safe = slug.replace('/', "__");
    snapshots_dir.join(format!("{safe}.json"))
}

pub fn load(snapshots_dir: &Path, slug: &str) -> Option<Snapshot> {
    let path = sidecar_path(snapshots_dir, slug);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save(snapshots_dir: &Path, snap: &Snapshot) -> std::io::Result<()> {
    fs::create_dir_all(snapshots_dir)?;
    let path = sidecar_path(snapshots_dir, &snap.slug);
    let body = serde_json::to_string_pretty(snap)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(path, body)
}

pub fn diff(prev: Option<&Snapshot>, cur: &Snapshot) -> SnapshotDiff {
    let mut d = SnapshotDiff::default();
    let prev_files = prev.map(|s| &s.files);
    for (path, entry) in &cur.files {
        match prev_files.and_then(|p| p.get(path)) {
            None => d.added.push(path.clone()),
            Some(prev_entry) if prev_entry == entry => d.unchanged.push(path.clone()),
            Some(_) => d.changed.push(path.clone()),
        }
    }
    if let Some(prev_map) = prev_files {
        for path in prev_map.keys() {
            if !cur.files.contains_key(path) {
                d.removed.push(path.clone());
            }
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_file(dir: &Path, name: &str, body: &[u8]) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn capture_skips_metadata_file() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.bin", b"hello");
        write_file(tmp.path(), "dataset-metadata.json", b"{}");
        let snap = capture(tmp.path(), "u/x").unwrap();
        assert!(snap.files.contains_key("a.bin"));
        assert!(!snap.files.contains_key("dataset-metadata.json"));
    }

    #[test]
    fn diff_reports_added_changed_removed() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "stable.bin", b"a");
        write_file(tmp.path(), "removed.bin", b"b");
        let prev = capture(tmp.path(), "u/x").unwrap();

        // Mutate: change `removed.bin` away, change `stable.bin` size, add new.
        fs::remove_file(tmp.path().join("removed.bin")).unwrap();
        write_file(tmp.path(), "stable.bin", b"abcdef"); // size changed
        write_file(tmp.path(), "new.bin", b"new");

        let cur = capture(tmp.path(), "u/x").unwrap();
        let d = diff(Some(&prev), &cur);
        assert_eq!(d.added, vec!["new.bin"]);
        assert_eq!(d.changed, vec!["stable.bin"]);
        assert_eq!(d.removed, vec!["removed.bin"]);
        assert!(d.unchanged.is_empty());
        assert!(!d.is_empty());
    }

    #[test]
    fn diff_against_no_prev_marks_all_added() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.bin", b"x");
        let snap = capture(tmp.path(), "u/x").unwrap();
        let d = diff(None, &snap);
        assert_eq!(d.added, vec!["a.bin"]);
        assert!(d.unchanged.is_empty());
        assert!(!d.is_empty());
    }

    #[test]
    fn save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.bin", b"x");
        let snap = capture(tmp.path(), "user/dataset").unwrap();
        save(tmp.path(), &snap).unwrap();
        let loaded = load(tmp.path(), "user/dataset").unwrap();
        assert_eq!(loaded.files, snap.files);
        assert_eq!(loaded.slug, "user/dataset");
    }
}
