#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventStatus {
    Start,
    Ok,
    Fail,
    Progress,
}

impl<'de> Deserialize<'de> for EventStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "start" => Self::Start,
            "ok" => Self::Ok,
            "fail" => Self::Fail,
            "progress" => Self::Progress,
            other => {
                tracing::warn!(status = other, "unknown event status, treating as progress");
                Self::Progress
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub ts: DateTime<Utc>,
    pub stage: String,
    pub status: EventStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

