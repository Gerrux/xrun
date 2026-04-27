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

/// Standard stage names for poller-side classification.
///
/// `stage` in `Event` is a plain `String`; this enum exists solely for
/// `is_terminal` / `is_stage_done` helpers on the poller side.
#[derive(Debug, Clone, PartialEq)]
pub enum StdStage {
    Provision,
    Upload,
    Unpack,
    EnvReady,
    TrainStart,
    Epoch,
    TrainEnd,
    ArtifactsReady,
    Done,
    Unknown(String),
}

impl StdStage {
    pub fn from_stage(s: &str) -> Self {
        match s {
            "provision" => Self::Provision,
            "upload" => Self::Upload,
            "unpack" => Self::Unpack,
            "env_ready" => Self::EnvReady,
            "train_start" => Self::TrainStart,
            "epoch" => Self::Epoch,
            "train_end" => Self::TrainEnd,
            "artifacts_ready" => Self::ArtifactsReady,
            "done" => Self::Done,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done)
    }

    pub fn is_stage_done(&self) -> bool {
        matches!(self, Self::Done | Self::TrainEnd | Self::ArtifactsReady)
    }
}
