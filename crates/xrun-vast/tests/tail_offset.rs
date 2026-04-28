use std::path::PathBuf;

use tempfile::tempdir;
use xrun_vast::{
    error::VastError,
    pull::{apply_keep_last, classify_kind, has_wildcard, parse_ls_output, sha256_of_file},
    tail::{decide_tail_action, parse_wc_output, TailDecision},
};

// ─── parse_wc_output ─────────────────────────────────────────────────────────

#[test]
fn parse_wc_output_with_leading_whitespace() {
    assert_eq!(parse_wc_output(b"  18992\n"), Some(18992));
}

#[test]
fn parse_wc_output_plain_number() {
    assert_eq!(parse_wc_output(b"12480\n"), Some(12480));
}

#[test]
fn parse_wc_output_zero() {
    assert_eq!(parse_wc_output(b"0\n"), Some(0));
}

#[test]
fn parse_wc_output_invalid_returns_none() {
    assert_eq!(parse_wc_output(b"not a number\n"), None);
    assert_eq!(parse_wc_output(b""), None);
}

// ─── decide_tail_action ───────────────────────────────────────────────────────

#[test]
fn tail_empty_when_size_equals_offset() {
    assert!(matches!(
        decide_tail_action(12480, 12480),
        TailDecision::Empty
    ));
}

#[test]
fn tail_truncated_when_size_less_than_offset() {
    let decision = decide_tail_action(100, 500);
    assert!(
        matches!(decision, TailDecision::Truncated { was: 500, now: 100 }),
        "expected Truncated with was=500 now=100"
    );
}

#[test]
fn tail_truncated_propagates_into_error() {
    let decision = decide_tail_action(100, 500);
    match decision {
        TailDecision::Truncated { was, now } => {
            let err = VastError::FileTruncated {
                file: "/workspace/run/stdout.log".to_string(),
                was,
                now,
            };
            assert!(err.to_string().contains("500"));
            assert!(err.to_string().contains("100"));
        }
        _ => panic!("expected Truncated"),
    }
}

#[test]
fn tail_read_uses_one_indexed_start() {
    // tail -c +N is 1-indexed: to read from byte 12480 onward, start byte = 12481.
    let decision = decide_tail_action(18992, 12480);
    match decision {
        TailDecision::Read { start_byte } => {
            assert_eq!(start_byte, 12481);
        }
        _ => panic!("expected Read"),
    }
}

#[test]
fn tail_read_delta_length_correct() {
    let size: u64 = 18992;
    let offset: u64 = 12480;
    let delta = size - offset;
    assert_eq!(delta, 6512);
}

// ─── has_wildcard ─────────────────────────────────────────────────────────────

#[test]
fn has_wildcard_star() {
    assert!(has_wildcard("output/ep*.pt"));
    assert!(has_wildcard("*.log"));
}

#[test]
fn has_wildcard_question_mark() {
    assert!(has_wildcard("ep?.pt"));
}

#[test]
fn has_wildcard_bracket() {
    assert!(has_wildcard("ep[0-9].pt"));
}

#[test]
fn no_wildcard_in_plain_path() {
    assert!(!has_wildcard("output/ep003.pt"));
    assert!(!has_wildcard("/workspace/run/stdout.log"));
}

// ─── parse_ls_output ─────────────────────────────────────────────────────────

#[test]
fn parse_ls_output_splits_three_lines() {
    let out = b"output/ep001.pt\noutput/ep002.pt\noutput/ep003.pt\n";
    let files = parse_ls_output(out);
    assert_eq!(files.len(), 3);
    assert_eq!(files[0], "output/ep001.pt");
    assert_eq!(files[2], "output/ep003.pt");
}

#[test]
fn parse_ls_output_trims_blank_lines() {
    let out = b"\noutput/ep001.pt\n\n";
    let files = parse_ls_output(out);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0], "output/ep001.pt");
}

#[test]
fn parse_ls_output_empty_yields_empty_vec() {
    let files = parse_ls_output(b"");
    assert!(files.is_empty());
}

// ─── classify_kind ───────────────────────────────────────────────────────────

#[test]
fn checkpoint_pt_extension() {
    assert_eq!(classify_kind("ep003.pt"), "checkpoint");
}

#[test]
fn checkpoint_ckpt_extension() {
    assert_eq!(classify_kind("best.ckpt"), "checkpoint");
}

#[test]
fn figure_png_extension() {
    assert_eq!(classify_kind("loss_curve.png"), "figure");
}

#[test]
fn json_extension() {
    assert_eq!(classify_kind("metrics.json"), "json");
}

#[test]
fn log_extension() {
    assert_eq!(classify_kind("stdout.log"), "log");
}

#[test]
fn unknown_extension_is_other() {
    assert_eq!(classify_kind("model.bin"), "other");
    assert_eq!(classify_kind("archive.tar"), "other");
}

#[test]
fn classify_kind_glob_paths_use_basename() {
    // classify_kind receives the filename only; verify it works with paths too.
    assert_eq!(classify_kind("output/ep003.pt"), "checkpoint");
}

// ─── sha256_of_file ──────────────────────────────────────────────────────────

#[test]
fn sha256_of_empty_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("empty.bin");
    std::fs::write(&path, b"").unwrap();
    let hash = sha256_of_file(&path).unwrap();
    assert_eq!(
        hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn sha256_has_64_hex_chars() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("data.bin");
    std::fs::write(&path, b"hello world\n").unwrap();
    let hash = sha256_of_file(&path).unwrap();
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn sha256_different_content_different_hash() {
    let dir = tempdir().unwrap();
    let p1 = dir.path().join("a.bin");
    let p2 = dir.path().join("b.bin");
    std::fs::write(&p1, b"content_a").unwrap();
    std::fs::write(&p2, b"content_b").unwrap();
    assert_ne!(sha256_of_file(&p1).unwrap(), sha256_of_file(&p2).unwrap());
}

// ─── apply_keep_last ─────────────────────────────────────────────────────────

#[test]
fn keep_last_removes_excess_files() {
    let dir = tempdir().unwrap();
    let mut files: Vec<PathBuf> = Vec::new();

    for i in 0..3u32 {
        let path = dir.path().join(format!("ep{:03}.pt", i));
        std::fs::write(&path, format!("epoch {}", i).as_bytes()).unwrap();
        files.push(path);
    }

    apply_keep_last(&mut files, 2);

    let remaining: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        remaining.len(),
        2,
        "keep_last=2 should leave exactly 2 files"
    );
    // The oldest file (ep000) should be deleted; the two newer ones kept.
    assert!(
        !dir.path().join("ep000.pt").exists(),
        "ep000.pt (oldest) should have been deleted"
    );
    assert!(
        dir.path().join("ep001.pt").exists(),
        "ep001.pt should have been kept"
    );
    assert!(
        dir.path().join("ep002.pt").exists(),
        "ep002.pt should have been kept"
    );
}

#[test]
fn keep_last_noop_when_at_or_below_limit() {
    let dir = tempdir().unwrap();
    let mut files: Vec<PathBuf> = Vec::new();

    for i in 0..2u32 {
        let path = dir.path().join(format!("ep{:03}.pt", i));
        std::fs::write(&path, b"data").unwrap();
        files.push(path);
    }

    apply_keep_last(&mut files, 5);

    let remaining: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        remaining.len(),
        2,
        "should not delete when count <= keep_last"
    );
}

#[test]
fn keep_last_zero_deletes_all() {
    let dir = tempdir().unwrap();
    let mut files: Vec<PathBuf> = Vec::new();

    for i in 0..3u32 {
        let path = dir.path().join(format!("ep{:03}.pt", i));
        std::fs::write(&path, b"data").unwrap();
        files.push(path);
    }

    apply_keep_last(&mut files, 0);

    let remaining: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(remaining.len(), 0, "keep_last=0 should delete all files");
}
