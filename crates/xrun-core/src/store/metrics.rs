#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};

use crate::error::StoreError;

use super::Store;

pub struct NewMetric {
    pub step: i64,
    pub key: String,
    pub value: f64,
    pub ts: DateTime<Utc>,
}

impl Store {
    pub fn append_metric(
        &mut self,
        run_id: &super::RunId,
        metric: NewMetric,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR REPLACE INTO metrics (run_id, step, key, value, ts) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, metric.step, metric.key, metric.value, metric.ts],
        )?;
        tx.commit()?;
        Ok(())
    }
}
