#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;

use super::{RunId, Store};

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

impl Store {
    pub fn insert_instance(
        &mut self,
        id: &str,
        vendor: &str,
        run_id: Option<&RunId>,
        gpu_type: Option<&str>,
        price_per_hour: Option<f64>,
        created_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let run_id_str = run_id.map(|r| r.to_string());
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO instances (id, vendor, run_id, gpu_type, price_per_hour, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, vendor, run_id_str, gpu_type, price_per_hour, created_at],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_instance_destroyed(
        &mut self,
        id: &str,
        destroyed_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE instances SET destroyed_at = ?1 WHERE id = ?2",
            params![destroyed_at, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_instance_state_json(
        &mut self,
        id: &str,
        state_json: &str,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE instances SET state_json = ?1 WHERE id = ?2",
            params![state_json, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_instance(&self, id: &str) -> Result<Option<Instance>, StoreError> {
        let sql = "SELECT id, vendor, run_id, gpu_type, price_per_hour, \
                   created_at, destroyed_at, state_json \
                   FROM instances WHERE id = ?1";
        match self.conn.query_row(sql, params![id], |row| {
            Ok(Instance {
                id: row.get(0)?,
                vendor: row.get(1)?,
                run_id: row.get(2)?,
                gpu_type: row.get(3)?,
                price_per_hour: row.get(4)?,
                created_at: row.get(5)?,
                destroyed_at: row.get(6)?,
                state_json: row.get(7)?,
            })
        }) {
            Ok(inst) => Ok(Some(inst)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    pub fn list_instances(&self) -> Result<Vec<Instance>, StoreError> {
        let sql = "SELECT id, vendor, run_id, gpu_type, price_per_hour, \
                   created_at, destroyed_at, state_json \
                   FROM instances ORDER BY created_at DESC";
        let mut stmt = self.conn.prepare(sql)?;
        let iter = stmt.query_map([], |row| {
            Ok(Instance {
                id: row.get(0)?,
                vendor: row.get(1)?,
                run_id: row.get(2)?,
                gpu_type: row.get(3)?,
                price_per_hour: row.get(4)?,
                created_at: row.get(5)?,
                destroyed_at: row.get(6)?,
                state_json: row.get(7)?,
            })
        })?;
        iter.collect::<Result<Vec<_>, _>>().map_err(StoreError::Db)
    }
}
