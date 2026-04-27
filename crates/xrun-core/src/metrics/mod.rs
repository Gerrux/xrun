#![deny(unsafe_code)]

pub mod types;

pub use types::Metric;

use std::io::BufRead;

use crate::error::JsonlError;

/// Iterator over a JSONL stream of [`Metric`] records.
///
/// Mirrors `JsonlReader`: bad lines yield `Err` without stopping iteration,
/// and `bytes_consumed()` tracks raw bytes for poller offset resumption.
pub struct MetricsJsonlReader<R: BufRead> {
    reader: R,
    bytes_consumed: u64,
}

impl<R: BufRead> MetricsJsonlReader<R> {
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

impl<R: BufRead> Iterator for MetricsJsonlReader<R> {
    type Item = Result<Metric, JsonlError>;

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
