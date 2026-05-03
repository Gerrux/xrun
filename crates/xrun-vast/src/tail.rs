#![deny(unsafe_code)]

use crate::{error::VastError, transfer::ssh_exec};

/// Decision made by comparing the remote file size to the current read offset.
pub enum TailDecision {
    /// No new bytes available.
    Empty,
    /// File shrank — likely a pre-emption restart.
    Truncated { was: u64, now: u64 },
    /// New bytes available; `start_byte` is 1-indexed for `tail -c +N`.
    Read { start_byte: u64 },
}

/// Compute the tail action given the current remote file size and local offset.
pub fn decide_tail_action(file_size: u64, offset: u64) -> TailDecision {
    if file_size == offset {
        TailDecision::Empty
    } else if file_size < offset {
        TailDecision::Truncated {
            was: offset,
            now: file_size,
        }
    } else {
        TailDecision::Read {
            start_byte: offset + 1,
        }
    }
}

/// Parse `wc -c < file` stdout (e.g. b"  18992\n") into a byte count.
pub fn parse_wc_output(bytes: &[u8]) -> Option<u64> {
    let s = std::str::from_utf8(bytes).ok()?;
    s.trim().parse::<u64>().ok()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Incrementally read bytes from `file` starting at `offset`.
///
/// Returns an empty vec if the file hasn't grown. Returns `FileTruncated` when
/// the file shrank (pre-emption restart). Otherwise returns the new bytes.
pub async fn tail_file(
    host: &str,
    port: u16,
    file: &str,
    offset: u64,
) -> Result<Vec<u8>, VastError> {
    let quoted = shell_quote(file);
    let wc_cmd = format!("wc -c < {}", quoted);
    let wc_out = ssh_exec(host, port, &wc_cmd).await?;
    let size = parse_wc_output(&wc_out)
        .ok_or_else(|| VastError::ParseError(format!("unexpected wc output: {:?}", wc_out)))?;

    match decide_tail_action(size, offset) {
        TailDecision::Empty => Ok(vec![]),
        TailDecision::Truncated { was, now } => Err(VastError::FileTruncated {
            file: file.to_string(),
            was,
            now,
        }),
        TailDecision::Read { start_byte } => {
            let tail_cmd = format!("tail -c +{} {}", start_byte, quoted);
            ssh_exec(host, port, &tail_cmd).await
        }
    }
}
