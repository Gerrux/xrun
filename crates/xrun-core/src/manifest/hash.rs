#![deny(unsafe_code)]

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write as FmtWrite;

use super::types::Manifest;

impl Manifest {
    pub fn canonical_hash(&self) -> String {
        let v = serde_json::to_value(self).expect("manifest serialization is infallible");
        let canonical = sort_and_clean(v);
        let s = serde_json::to_string(&canonical).expect("JSON serialization is infallible");
        let digest = Sha256::digest(s.as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest.iter() {
            write!(hex, "{:02x}", byte).unwrap();
        }
        hex
    }
}

fn sort_and_clean(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<String, Value> = map
                .into_iter()
                .filter(|(_, val)| !val.is_null())
                .map(|(k, val)| (k, sort_and_clean(val)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sort_and_clean).collect()),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                Value::Number(serde_json::Number::from_f64(f).unwrap_or(n))
            } else {
                Value::Number(n)
            }
        }
        other => other,
    }
}
