#![deny(unsafe_code)]

mod artifacts;
mod events;
mod instances;
mod metrics;
mod poll_offsets;
mod runs;

pub use artifacts::NewArtifact;
pub use events::{NewEvent, StoredEvent};
pub use instances::{Instance, InstanceCaps};
pub use metrics::{NewMetric, StoredMetric};
pub use runs::{ListFilter, Run, RunId, RunStatus};

use crate::error::StoreError;
use rusqlite::{Connection, TransactionBehavior};
use std::path::Path;

const CURRENT_SCHEMA_VERSION: u32 = 5;
const MIGRATION_001: &str = include_str!("migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("migrations/002_cost_estimate.sql");
const MIGRATION_003: &str = include_str!("migrations/003_budget.sql");
const MIGRATION_004: &str = include_str!("migrations/004_poller_pid.sql");
const MIGRATION_005: &str = include_str!("migrations/005_sink_run_ids.sql");

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let mut store = Store { conn };
        store.apply_migrations()?;
        Ok(store)
    }

    fn apply_migrations(&mut self) -> Result<(), StoreError> {
        let count: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get(0),
        )?;

        if count == 0 {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(MIGRATION_001)?;
            tx.execute_batch(MIGRATION_002)?;
            tx.execute_batch(MIGRATION_003)?;
            tx.execute_batch(MIGRATION_004)?;
            tx.execute_batch(MIGRATION_005)?;
            tx.commit()?;
        } else {
            let version: u32 =
                self.conn
                    .query_row("SELECT version FROM schema_version", [], |row| row.get(0))?;
            if version > CURRENT_SCHEMA_VERSION {
                return Err(StoreError::SchemaTooNew {
                    found: version,
                    supported: CURRENT_SCHEMA_VERSION,
                });
            }
            if version < 2 {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(MIGRATION_002)?;
                tx.commit()?;
            }
            if version < 3 {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(MIGRATION_003)?;
                tx.commit()?;
            }
            if version < 4 {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(MIGRATION_004)?;
                tx.commit()?;
            }
            if version < 5 {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(MIGRATION_005)?;
                tx.commit()?;
            }
        }
        Ok(())
    }
}
