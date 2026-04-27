#![deny(unsafe_code)]

use chrono::Utc;
use rusqlite::{params, TransactionBehavior};

use crate::error::StoreError;

use super::Store;

impl Store {
    pub fn update_poll_offset(
        &mut self,
        run_id: &super::RunId,
        file: &str,
        offset: u64,
    ) -> Result<(), StoreError> {
        #[allow(clippy::cast_possible_wrap)]
        let offset_i64 = offset as i64;
        let now = Utc::now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO poll_offsets (run_id, file, offset_bytes, last_polled_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(run_id, file) DO UPDATE SET \
               offset_bytes = excluded.offset_bytes, \
               last_polled_at = excluded.last_polled_at",
            params![run_id, file, offset_i64, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_poll_offset(&self, run_id: &super::RunId, file: &str) -> Result<u64, StoreError> {
        let result = self.conn.query_row(
            "SELECT offset_bytes FROM poll_offsets WHERE run_id = ?1 AND file = ?2",
            params![run_id, file],
            |row| row.get::<_, i64>(0),
        );
        match result {
            Ok(v) =>
            {
                #[allow(clippy::cast_sign_loss)]
                Ok(v as u64)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(StoreError::Db(e)),
        }
    }
}
