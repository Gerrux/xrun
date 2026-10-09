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

/// One file as Kaggle lists it for the current dataset version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// Path inside the dataset, `/`-separated, no leading slash.
    pub name: String,
    /// Kaggle's `totalBytes`; `None` when the API omits it.
    pub total_bytes: Option<u64>,
}

/// Result of checking a just-pushed staging dir against what Kaggle lists.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RemoteCheck {
    pub local_count: usize,
    pub local_bytes: u64,
    pub remote_count: usize,
    pub remote_bytes: u64,
    /// Local paths Kaggle does not list.
    pub missing: Vec<String>,
    /// Paths Kaggle lists that are not in the staging dir (for example an
    /// archive it did not extract).
    pub extra: Vec<String>,
}

impl RemoteCheck {
    /// Every local file is present remotely. Extras are reported but do not
    /// fail the check; byte totals are informational only (Kaggle may count
    /// differently from the local filesystem).
    pub fn ok(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Compare the staging snapshot with Kaggle's file list for the same version.
///
/// Names are compared as `/`-separated relative paths; `dataset-metadata.json`
/// is never in the snapshot and is ignored on the remote side as well.
pub fn compare_remote(local: &Snapshot, remote: &[RemoteFile]) -> RemoteCheck {
    let remote_names: std::collections::BTreeSet<&str> = remote
        .iter()
        .map(|f| f.name.trim_start_matches('/'))
        .filter(|n| *n != "dataset-metadata.json")
        .collect();
    let mut check = RemoteCheck {
        local_count: local.files.len(),
        local_bytes: local.files.values().map(|e| e.size).sum(),
        remote_count: remote_names.len(),
        remote_bytes: remote.iter().filter_map(|f| f.total_bytes).sum(),
        ..RemoteCheck::default()
    };
    for path in local.files.keys() {
        if !remote_names.contains(path.as_str()) {
            check.missing.push(path.clone());
        }
    }
    for name in remote_names {
        if !local.files.contains_key(name) {
            check.extra.push(name.to_string());
        }
    }
    check
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

    fn snap(files: &[(&str, u64)]) -> Snapshot {
        Snapshot {
            slug: "u/x".into(),
            captured_at: String::new(),
            files: files
                .iter()
                .map(|(p, s)| (p.to_string(), FileEntry { size: *s, mtime: 0 }))
                .collect(),
        }
    }

    fn remote(names: &[(&str, Option<u64>)]) -> Vec<RemoteFile> {
        names
            .iter()
            .map(|(n, b)| RemoteFile {
                name: n.to_string(),
                total_bytes: *b,
            })
            .collect()
    }

    #[test]
    fn compare_remote_matches_when_every_local_path_is_listed() {
        let local = snap(&[("train/a.npz", 10), ("val/b.npz", 20)]);
        let check = compare_remote(
            &local,
            &remote(&[("train/a.npz", Some(10)), ("/val/b.npz", Some(20))]),
        );
        assert!(check.ok(), "{check:?}");
        assert_eq!(check.local_count, 2);
        assert_eq!(check.remote_count, 2);
        assert_eq!(check.remote_bytes, 30);
        assert!(check.extra.is_empty());
    }

    #[test]
    fn compare_remote_flags_empty_version() {
        // The powerline-seg-v1 incident: CLI exit 0, status `ready`, zero files.
        let local = snap(&[("train/a.npz", 10), ("val/b.npz", 20)]);
        let check = compare_remote(&local, &[]);
        assert!(!check.ok());
        assert_eq!(check.missing, vec!["train/a.npz", "val/b.npz"]);
        assert_eq!(check.remote_count, 0);
    }

    #[test]
    fn compare_remote_reports_unextracted_archive_as_extra() {
        let local = snap(&[("train/a.npz", 10), ("val/b.npz", 20)]);
        let check = compare_remote(
            &local,
            &remote(&[("train.tar", Some(30)), ("val.tar", None)]),
        );
        assert!(!check.ok());
        assert_eq!(check.missing.len(), 2);
        assert_eq!(check.extra, vec!["train.tar", "val.tar"]);
        // `None` bytes are skipped, not treated as zero-and-failing.
        assert_eq!(check.remote_bytes, 30);
    }

    #[test]
    fn compare_remote_ignores_remote_metadata_file() {
        let local = snap(&[("a.bin", 1)]);
        let check = compare_remote(
            &local,
            &remote(&[("a.bin", Some(1)), ("dataset-metadata.json", Some(99))]),
        );
        assert!(check.ok());
        assert!(check.extra.is_empty());
        assert_eq!(check.remote_count, 1);
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
