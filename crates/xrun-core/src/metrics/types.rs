#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub ts: DateTime<Utc>,
    pub step: i64,
    pub key: String,
    pub value: f64,
}
