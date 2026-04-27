#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub vendor: String,
    pub run_id: Option<String>,
    pub gpu_type: Option<String>,
    pub price_per_hour: Option<f64>,
    pub created_at: Option<DateTime<Utc>>,
    pub destroyed_at: Option<DateTime<Utc>>,
    pub state_json: Option<String>,
}
