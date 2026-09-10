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
                    return Some(match serde_json::from_str::<Metric>(trimmed) {
                        Ok(m) => Ok(m),
                        Err(e) => parse_lenient(trimmed).ok_or(JsonlError::Json(e)),
                    });
                }
                Err(e) => return Some(Err(JsonlError::Io(e))),
            }
        }
    }
}

/// Python's `json.dumps` writes `NaN` / `Infinity` / `-Infinity` bare, which
/// strict JSON (and serde_json) rejects. A NaN loss is exactly the point
/// where you want to be told, so recover those lines instead of dropping
/// them. Only the `value` field is allowed to be non-finite; the caller
/// decides what to do with such a metric (the store never persists it).
fn parse_lenient(line: &str) -> Option<Metric> {
    if !(line.contains("NaN") || line.contains("Infinity")) {
        return None;
    }
    let patched = line
        .replace("-Infinity", "\"-Infinity\"")
        .replace(":Infinity", ":\"Infinity\"")
        .replace(": Infinity", ": \"Infinity\"")
        .replace(":NaN", ":\"NaN\"")
        .replace(": NaN", ": \"NaN\"");
    let v: serde_json::Value = serde_json::from_str(&patched).ok()?;
    let value = match v.get("value")? {
        serde_json::Value::Number(n) => n.as_f64()?,
        serde_json::Value::String(s) => match s.as_str() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => return None,
        },
        _ => return None,
    };
    let ts_str = v.get("ts")?.as_str()?;
    let ts = chrono::DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&chrono::Utc);
    Some(Metric {
        ts,
        step: v.get("step")?.as_i64()?,
        key: v.get("key")?.as_str()?.to_string(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn strict_lines_parse_and_nan_lines_are_recovered() {
        let data = b"{\"ts\":\"2026-01-01T00:00:00Z\",\"step\":1,\"key\":\"loss\",\"value\":0.5}
                     {\"ts\":\"2026-01-01T00:00:01Z\",\"step\":2,\"key\":\"loss\",\"value\":NaN}
                     {\"ts\":\"2026-01-01T00:00:02Z\",\"step\":3,\"key\":\"loss\",\"value\": -Infinity}
                     {\"ts\":\"2026-01-01T00:00:03Z\",\"step\":4,\"key\":\"loss\",\"value\":\"oops\"}
";
        let items: Vec<_> = MetricsJsonlReader::new(BufReader::new(&data[..])).collect();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].as_ref().unwrap().value, 0.5);
        assert!(items[1].as_ref().unwrap().value.is_nan());
        assert_eq!(items[2].as_ref().unwrap().value, f64::NEG_INFINITY);
        assert!(items[3].is_err(), "a string value is still malformed");
    }
}
