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
    pub max_lifetime_secs: Option<i64>,
    pub max_cost_usd: Option<f64>,
    pub idle_timeout_secs: Option<i64>,
    pub accumulated_cost: f64,
    pub last_active_at: Option<DateTime<Utc>>,
    pub auto_destroyed_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InstanceCaps {
    pub max_lifetime_secs: Option<i64>,
    pub max_cost_usd: Option<f64>,
    pub idle_timeout_secs: Option<i64>,
}

const SELECT_COLS: &str = "id, vendor, run_id, gpu_type, price_per_hour, \
     created_at, destroyed_at, state_json, \
     max_lifetime_secs, max_cost_usd, idle_timeout_secs, \
     accumulated_cost, last_active_at, auto_destroyed_reason";

fn map_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<Instance> {
    Ok(Instance {
        id: row.get(0)?,
        vendor: row.get(1)?,
        run_id: row.get(2)?,
        gpu_type: row.get(3)?,
        price_per_hour: row.get(4)?,
        created_at: row.get(5)?,
        destroyed_at: row.get(6)?,
        state_json: row.get(7)?,
        max_lifetime_secs: row.get(8)?,
        max_cost_usd: row.get(9)?,
        idle_timeout_secs: row.get(10)?,
        accumulated_cost: row.get(11)?,
        last_active_at: row.get(12)?,
        auto_destroyed_reason: row.get(13)?,
    })
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
        self.insert_instance_with_caps(
            id,
            vendor,
            run_id,
            gpu_type,
            price_per_hour,
            created_at,
            &InstanceCaps::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_instance_with_caps(
        &mut self,
        id: &str,
        vendor: &str,
        run_id: Option<&RunId>,
        gpu_type: Option<&str>,
        price_per_hour: Option<f64>,
        created_at: DateTime<Utc>,
        caps: &InstanceCaps,
    ) -> Result<(), StoreError> {
        let run_id_str = run_id.map(|r| r.to_string());
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO instances \
             (id, vendor, run_id, gpu_type, price_per_hour, created_at, \
              max_lifetime_secs, max_cost_usd, idle_timeout_secs) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                vendor,
                run_id_str,
                gpu_type,
                price_per_hour,
                created_at,
                caps.max_lifetime_secs,
                caps.max_cost_usd,
                caps.idle_timeout_secs,
            ],
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

    /// Update accumulated cost and (optionally) `last_active_at`. Idempotent —
    /// callers can update the cost on every poll tick without producing
    /// duplicate rows. Pass `last_active_at = None` to leave it unchanged.
    pub fn update_instance_usage(
        &mut self,
        id: &str,
        accumulated_cost: f64,
        last_active_at: Option<DateTime<Utc>>,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(active) = last_active_at {
            tx.execute(
                "UPDATE instances SET accumulated_cost = ?1, last_active_at = ?2 WHERE id = ?3",
                params![accumulated_cost, active, id],
            )?;
        } else {
            tx.execute(
                "UPDATE instances SET accumulated_cost = ?1 WHERE id = ?2",
                params![accumulated_cost, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Mark the instance as auto-destroyed for budget reasons. Written *before*
    /// the destroy API call so that a daemon restart does not double-destroy
    /// (the next tick will see `auto_destroyed_reason IS NOT NULL` and skip).
    pub fn set_auto_destroyed_reason(
        &mut self,
        id: &str,
        reason: &str,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE instances SET auto_destroyed_reason = ?1 WHERE id = ?2",
            params![reason, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_instance(&self, id: &str) -> Result<Option<Instance>, StoreError> {
        let sql = format!("SELECT {SELECT_COLS} FROM instances WHERE id = ?1");
        match self.conn.query_row(&sql, params![id], map_instance) {
            Ok(inst) => Ok(Some(inst)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    pub fn list_instances(&self) -> Result<Vec<Instance>, StoreError> {
        let sql = format!("SELECT {SELECT_COLS} FROM instances ORDER BY created_at DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let iter = stmt.query_map([], map_instance)?;
        iter.collect::<Result<Vec<_>, _>>().map_err(StoreError::Db)
    }

    /// Active = not yet destroyed and not flagged for auto-destroy. Used by the
    /// poll-daemon to decide which instances to evaluate caps on.
    pub fn list_active_instances(&self) -> Result<Vec<Instance>, StoreError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM instances \
             WHERE destroyed_at IS NULL AND auto_destroyed_reason IS NULL \
             ORDER BY created_at ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let iter = stmt.query_map([], map_instance)?;
        iter.collect::<Result<Vec<_>, _>>().map_err(StoreError::Db)
    }
}
