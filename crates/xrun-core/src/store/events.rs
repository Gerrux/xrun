#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};
use serde::Serialize;

use crate::error::StoreError;

use super::Store;

pub struct NewEvent {
    pub ts: DateTime<Utc>,
    pub stage: String,
    pub status: String,
    pub msg: Option<String>,
    pub payload_json: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredEvent {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub stage: String,
    pub status: String,
    pub msg: Option<String>,
    pub payload_json: Option<String>,
}

impl Store {
    pub fn append_event(
        &mut self,
        run_id: &super::RunId,
        event: NewEvent,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO events (run_id, ts, stage, status, msg, payload_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run_id,
                event.ts,
                event.stage,
                event.status,
                event.msg,
                event.payload_json
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_events(&self, run_id: &super::RunId) -> Result<Vec<StoredEvent>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, ts, stage, status, msg, payload_json \
             FROM events WHERE run_id = ?1 ORDER BY ts",
        )?;
        let rows: rusqlite::Result<Vec<StoredEvent>> = stmt
            .query_map([run_id], |row| {
                Ok(StoredEvent {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    stage: row.get(2)?,
                    status: row.get(3)?,
                    msg: row.get(4)?,
                    payload_json: row.get(5)?,
                })
            })?
            .collect();
        Ok(rows?)
    }
}
