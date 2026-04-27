#![deny(unsafe_code)]

use std::io::BufRead;

use crate::error::JsonlError;
use crate::events::types::Event;

/// Iterator over a JSONL stream of [`Event`] records.
///
/// Bad lines yield `Err` but do not stop iteration; the next call to `next()`
/// resumes at the following line.  `bytes_consumed()` tracks raw bytes read
/// regardless of parse success, which the poller uses to resume from an offset.
pub struct JsonlReader<R: BufRead> {
    reader: R,
    bytes_consumed: u64,
}

impl<R: BufRead> JsonlReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            bytes_consumed: 0,
        }
    }

    pub fn bytes_consumed(&self) -> u64 {
        self.bytes_consumed
    }
}

impl<R: BufRead> Iterator for JsonlReader<R> {
    type Item = Result<Event, JsonlError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(n) => {
                    self.bytes_consumed += n as u64;
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    return Some(serde_json::from_str(trimmed).map_err(JsonlError::Json));
                }
                Err(e) => return Some(Err(JsonlError::Io(e))),
            }
        }
    }
}
