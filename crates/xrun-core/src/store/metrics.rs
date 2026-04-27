#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};
use serde::Serialize;

use crate::error::StoreError;

use super::Store;

pub struct NewMetric {
    pub step: i64,
    pub key: String,
    pub value: f64,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredMetric {
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

    pub fn list_metrics(
        &self,
        run_id: &super::RunId,
        keys: Option<&[String]>,
    ) -> Result<Vec<StoredMetric>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT step, key, value, ts FROM metrics \
             WHERE run_id = ?1 ORDER BY step, key",
        )?;
        let all: rusqlite::Result<Vec<StoredMetric>> = stmt
            .query_map([run_id], |row| {
                Ok(StoredMetric {
                    step: row.get(0)?,
                    key: row.get(1)?,
                    value: row.get(2)?,
                    ts: row.get(3)?,
                })
            })?
            .collect();
        let all = all?;
        Ok(match keys {
            Some(filter) if !filter.is_empty() => all
                .into_iter()
                .filter(|m| filter.contains(&m.key))
                .collect(),
            _ => all,
        })
    }

    pub fn list_metric_keys(
        &self,
        run_id: &super::RunId,
    ) -> Result<Vec<(String, i64)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT key, COUNT(*) as cnt FROM metrics \
             WHERE run_id = ?1 GROUP BY key ORDER BY key",
        )?;
        let rows: rusqlite::Result<Vec<(String, i64)>> = stmt
            .query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        Ok(rows?)
    }
}
