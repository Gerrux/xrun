#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};

use crate::error::StoreError;

use super::Store;

pub struct NewEvent {
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
}
