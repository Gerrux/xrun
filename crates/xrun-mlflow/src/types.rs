#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};

pub type ExperimentId = String;
pub type RunId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlflowMetric {
    pub key: String,
    pub value: f64,
    pub timestamp: i64,
    pub step: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlflowParam {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlflowTag {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    Running,
    Scheduled,
    Finished,
    Failed,
    Killed,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Running => "RUNNING",
            RunStatus::Scheduled => "SCHEDULED",
            RunStatus::Finished => "FINISHED",
            RunStatus::Failed => "FAILED",
            RunStatus::Killed => "KILLED",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Experiment {
    pub experiment_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MlflowRun {
    pub info: RunInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunInfo {
    pub run_id: String,
    pub experiment_id: String,
    pub status: String,
}
