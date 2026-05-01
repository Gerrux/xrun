#![deny(unsafe_code)]

//! Chunk-reassembly helpers shared by the Kaggle adapter's live `tail()`.
//!
//! The xrun_hook log streamer (Python, runs inside the Kaggle kernel) pushes
//! stdout chunks to MLflow as `logs/log_NNNNNN.txt` artifacts, monotonically
//! numbered. The adapter pulls them back here, sorts by sequence, and serves
//! the slice past the poller's offset.
//!
//! Pure logic only — no HTTP, no I/O. The heavy lifting lives in
//! `adapter.rs::KaggleAdapter::tail`.

/// Tag key the streamer writes so the adapter can find the MLflow run that
/// corresponds to a given xrun run_id.
pub const TAG_RUN_ID: &str = "xrun_run_id";

/// Default MLflow experiment name used by both ends. Must stay in sync with
/// `_log_streamer.py::DEFAULT_EXPERIMENT`.
pub const LOG_STREAM_EXPERIMENT: &str = "xrun-logs";

/// Filename of the in-kernel stdout log the streamer tails.
pub const LOG_STREAM_FILE: &str = "__xrun_stdout.log";

/// Prefix under which chunks live in MLflow artifact storage.
pub const ARTIFACT_PREFIX: &str = "logs";

/// Parse the chunk sequence number from a path like `logs/log_000007.txt`.
/// Returns None for any other shape (sub-directories, non-chunk artifacts) so
/// callers can ignore them rather than panic.
pub fn parse_chunk_seq(path: &str) -> Option<u32> {
    let stem = path.rsplit('/').next()?;
    let stem = stem.strip_suffix(".txt")?;
    let digits = stem.strip_prefix("log_")?;
    digits.parse::<u32>().ok()
}

/// Extract the bytes the poller hasn't seen yet given an ordered list of
/// chunks and a callback that downloads a single chunk by index.
///
/// `chunks` is `(seq, path, size_bytes)` sorted by `seq`. `offset` is the
/// poller's cumulative byte offset across the full reassembled stream.
///
/// `download(idx)` is invoked at most once per chunk that overlaps
/// `[offset, total)`; chunks fully before `offset` are skipped without a
/// download. Errors propagate back to the caller.
pub fn slice_from_offset<E>(
    chunks: &[(u32, String, u64)],
    offset: u64,
    mut download: impl FnMut(usize) -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, E> {
    let total: u64 = chunks.iter().map(|(_, _, s)| *s).sum();
    if offset >= total {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut cum: u64 = 0;
    for (idx, (_, _, size)) in chunks.iter().enumerate() {
        let chunk_end = cum + *size;
        if chunk_end <= offset {
            cum = chunk_end;
            continue;
        }
        let bytes = download(idx)?;
        if cum >= offset {
            out.extend_from_slice(&bytes);
        } else {
            // chunk straddles the offset — skip the prefix that's already
            // been served on a previous tick.
            let skip = (offset - cum) as usize;
            if skip < bytes.len() {
                out.extend_from_slice(&bytes[skip..]);
            }
        }
        cum = chunk_end;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chunk_seq_accepts_six_digit_zero_padded() {
        assert_eq!(parse_chunk_seq("logs/log_000001.txt"), Some(1));
        assert_eq!(parse_chunk_seq("logs/log_000123.txt"), Some(123));
        assert_eq!(parse_chunk_seq("logs/log_999999.txt"), Some(999_999));
    }

    #[test]
    fn parse_chunk_seq_accepts_more_than_six_digits() {
        // Defensive: if a run streams enough chunks to overflow 6 digits we
        // still want monotonic ordering rather than silently dropping them.
        assert_eq!(parse_chunk_seq("logs/log_1234567.txt"), Some(1_234_567));
    }

    #[test]
    fn parse_chunk_seq_rejects_non_chunk_paths() {
        assert_eq!(parse_chunk_seq("logs/subdir/inner.txt"), None);
        assert_eq!(parse_chunk_seq("logs/README.md"), None);
        assert_eq!(parse_chunk_seq("logs/log_abc.txt"), None);
        assert_eq!(parse_chunk_seq("logs/log_1.json"), None);
    }

    fn fake_chunks(sizes: &[u64]) -> Vec<(u32, String, u64)> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let seq = (i + 1) as u32;
                (seq, format!("logs/log_{:06}.txt", seq), *s)
            })
            .collect()
    }

    #[test]
    fn slice_returns_empty_when_offset_at_eof() {
        let chunks = fake_chunks(&[10, 5]);
        let calls = std::cell::RefCell::new(0usize);
        let bytes = slice_from_offset::<()>(&chunks, 15, |_| {
            *calls.borrow_mut() += 1;
            Ok(vec![])
        })
        .unwrap();
        assert!(bytes.is_empty());
        assert_eq!(*calls.borrow(), 0, "no downloads when nothing past offset");
    }

    #[test]
    fn slice_skips_chunks_fully_before_offset() {
        let chunks = fake_chunks(&[10, 10, 10]);
        let downloads = std::cell::RefCell::new(Vec::<usize>::new());
        let bytes = slice_from_offset::<()>(&chunks, 20, |idx| {
            downloads.borrow_mut().push(idx);
            Ok(vec![b'C'; chunks[idx].2 as usize])
        })
        .unwrap();
        assert_eq!(bytes, vec![b'C'; 10]);
        assert_eq!(*downloads.borrow(), vec![2], "chunks 0,1 must be skipped");
    }

    #[test]
    fn slice_handles_offset_inside_chunk() {
        let chunks = fake_chunks(&[10, 10]);
        let bytes = slice_from_offset::<()>(&chunks, 7, |idx| match idx {
            0 => Ok(b"0123456789".to_vec()),
            1 => Ok(b"abcdefghij".to_vec()),
            _ => panic!("unexpected idx"),
        })
        .unwrap();
        // offset=7 → skip first 7 of chunk 0, then full chunk 1.
        assert_eq!(bytes, b"789abcdefghij");
    }

    #[test]
    fn slice_returns_full_when_offset_zero() {
        let chunks = fake_chunks(&[3, 4]);
        let bytes = slice_from_offset::<()>(&chunks, 0, |idx| match idx {
            0 => Ok(b"AAA".to_vec()),
            1 => Ok(b"BBBB".to_vec()),
            _ => panic!(),
        })
        .unwrap();
        assert_eq!(bytes, b"AAABBBB");
    }

    #[test]
    fn slice_propagates_download_error() {
        let chunks = fake_chunks(&[5]);
        let result: Result<Vec<u8>, &'static str> =
            slice_from_offset(&chunks, 0, |_| Err("boom"));
        assert_eq!(result, Err("boom"));
    }
}
