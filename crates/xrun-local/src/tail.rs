#![deny(unsafe_code)]

//! Local file tail — read everything past `offset`.
//!
//! Mirrors what the vast adapter does over SSH but on the local fs. A missing
//! file returns `Ok(empty)` so the poller just sees "no new bytes yet" until
//! the subprocess starts producing output.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::LocalError;

pub fn tail_file(path: &Path, offset: u64) -> Result<Vec<u8>, LocalError> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(LocalError::Io(e)),
    };
    let len = file.metadata()?.len();
    if offset >= len {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(offset))?;
    let to_read = (len - offset) as usize;
    let mut buf = Vec::with_capacity(to_read);
    file.take(to_read as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_empty() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("nope");
        let out = tail_file(&p, 0).expect("ok");
        assert!(out.is_empty());
    }

    #[test]
    fn reads_from_offset() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("log");
        std::fs::write(&p, b"hello world").unwrap();
        let out = tail_file(&p, 6).expect("ok");
        assert_eq!(out, b"world");
    }

    #[test]
    fn offset_past_eof_is_empty() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("log");
        std::fs::write(&p, b"hi").unwrap();
        let out = tail_file(&p, 100).expect("ok");
        assert!(out.is_empty());
    }

    #[test]
    fn incremental_tail_picks_up_appends() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("log");
        std::fs::write(&p, b"alpha").unwrap();
        let first = tail_file(&p, 0).expect("ok");
        assert_eq!(first, b"alpha");
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"beta").unwrap();
        drop(f);
        let next = tail_file(&p, first.len() as u64).expect("ok");
        assert_eq!(next, b"beta");
    }
}
