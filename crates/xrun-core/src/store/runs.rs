#![deny(unsafe_code)]

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{
    params,
    types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef},
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;

use super::Store;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub ulid::Ulid);

impl RunId {
    pub fn new() -> Self {
        RunId(ulid::Ulid::new())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for RunId {
    type Err = ulid::DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<ulid::Ulid>().map(RunId)
    }
}

impl ToSql for RunId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_string()))
    }
}

impl FromSql for RunId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = String::column_result(value)?;
        s.parse::<ulid::Ulid>()
            .map(RunId)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Provisioning,
    Uploading,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Provisioning => "provisioning",
            RunStatus::Uploading => "uploading",
            RunStatus::Running => "running",
            RunStatus::Done => "done",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
        }
    }
}

impl ToSql for RunStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for RunStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = String::column_result(value)?;
        match s.as_str() {
            "provisioning" => Ok(RunStatus::Provisioning),
            "uploading" => Ok(RunStatus::Uploading),
            "running" => Ok(RunStatus::Running),
            "done" => Ok(RunStatus::Done),
            "failed" => Ok(RunStatus::Failed),
            "cancelled" => Ok(RunStatus::Cancelled),
            other => Err(FromSqlError::Other(
                format!("unknown run status: {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub id: RunId,
    pub name: String,
    pub manifest_hash: String,
    pub manifest_path: String,
    pub vendor: String,
    pub instance_id: Option<String>,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub cost_usd: Option<f64>,
    pub mlflow_run_id: Option<String>,
    pub notes: Option<String>,
    /// PID of the detached poll-daemon, if one was spawned. None for foreground
    /// runs and for runs whose daemon was never started.
    pub poller_pid: Option<i64>,
}

#[derive(Default)]
pub struct ListFilter {
    pub status: Option<RunStatus>,
    pub vendor: Option<String>,
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: row.get(0)?,
        name: row.get(1)?,
        manifest_hash: row.get(2)?,
        manifest_path: row.get(3)?,
        vendor: row.get(4)?,
        instance_id: row.get(5)?,
        status: row.get(6)?,
        created_at: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        cost_usd: row.get(10)?,
        mlflow_run_id: row.get(11)?,
        notes: row.get(12)?,
        poller_pid: row.get(13)?,
    })
}

const SELECT_RUN_COLS: &str =
    "id, name, manifest_hash, manifest_path, vendor, instance_id, status, \
     created_at, started_at, ended_at, cost_usd, mlflow_run_id, notes, poller_pid";

impl Store {
    pub fn create_run(
        &mut self,
        name: &str,
        manifest_hash: &str,
        manifest_path: &str,
        vendor: &str,
        tags: &[String],
    ) -> Result<RunId, StoreError> {
        let id = RunId::new();
        let now = Utc::now();
        let notes: Option<String> = if tags.is_empty() {
            None
        } else {
            // Vec<String> serialization is always valid JSON
            Some(serde_json::to_string(tags).expect("Vec<String> serialization is infallible"))
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO runs (id, name, manifest_hash, manifest_path, vendor, status, created_at, notes) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, name, manifest_hash, manifest_path, vendor, RunStatus::Provisioning, now, notes],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn update_run_status(&mut self, id: &RunId, status: RunStatus) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows = tx.execute(
            "UPDATE runs SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        if rows == 0 {
            return Err(StoreError::RunNotFound(id.to_string()));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_run(&self, id: &RunId) -> Result<Option<Run>, StoreError> {
        let sql = format!("SELECT {SELECT_RUN_COLS} FROM runs WHERE id = ?1");
        match self.conn.query_row(&sql, params![id], row_to_run) {
            Ok(run) => Ok(Some(run)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    pub fn update_run_instance_id(
        &mut self,
        id: &RunId,
        instance_id: &str,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE runs SET instance_id = ?1 WHERE id = ?2",
            params![instance_id, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_run_started_at(
        &mut self,
        id: &RunId,
        started_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE runs SET started_at = ?1 WHERE id = ?2",
            params![started_at, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_run_cost(&mut self, id: &RunId, cost_usd: f64) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE runs SET cost_usd = ?1 WHERE id = ?2",
            params![cost_usd, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Sum of `cost_usd_estimate` for runs created on the given UTC date.
    /// Falls back to `cost_usd` when estimate is null. Used by budget.
    pub fn sum_run_cost_for_date(&self, day: NaiveDate) -> Result<f64, StoreError> {
        let day_start = day.and_hms_opt(0, 0, 0).expect("valid date");
        let day_end = day
            .succ_opt()
            .expect("date has successor")
            .and_hms_opt(0, 0, 0)
            .expect("valid date");
        let start = DateTime::<Utc>::from_naive_utc_and_offset(day_start, Utc);
        let end = DateTime::<Utc>::from_naive_utc_and_offset(day_end, Utc);
        let sum: Option<f64> = self.conn.query_row(
            "SELECT COALESCE(SUM(COALESCE(cost_usd_estimate, cost_usd, 0.0)), 0.0) \
             FROM runs WHERE created_at >= ?1 AND created_at < ?2",
            params![start, end],
            |row| row.get(0),
        )?;
        Ok(sum.unwrap_or(0.0))
    }

    /// Set the MLflow run ID for cross-linking with the MLflow UI.
    pub fn set_mlflow_run_id(&mut self, id: &RunId, mlflow_run_id: &str) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE runs SET mlflow_run_id = ?1 WHERE id = ?2",
            params![mlflow_run_id, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Record the PID of a detached poll-daemon. Pass `None` to clear the
    /// recorded PID (e.g. when the daemon exits cleanly or is reaped).
    pub fn update_run_poller_pid(
        &mut self,
        id: &RunId,
        pid: Option<i64>,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE runs SET poller_pid = ?1 WHERE id = ?2",
            params![pid, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_run_cost_estimate(
        &mut self,
        id: &RunId,
        estimate: f64,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE runs SET cost_usd_estimate = ?1 WHERE id = ?2",
            params![estimate, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Runs that are not in a terminal state (i.e. still in progress).
    pub fn list_active_runs(&self) -> Result<Vec<Run>, StoreError> {
        let sql = format!(
            "SELECT {SELECT_RUN_COLS} FROM runs \
             WHERE status NOT IN ('done', 'failed', 'cancelled') \
             ORDER BY created_at ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let iter = stmt.query_map([], row_to_run)?;
        iter.collect::<Result<Vec<_>, _>>().map_err(StoreError::Db)
    }

    pub fn list_runs(&self, filter: &ListFilter) -> Result<Vec<Run>, StoreError> {
        let sql = format!("SELECT {SELECT_RUN_COLS} FROM runs ORDER BY created_at DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let all: rusqlite::Result<Vec<Run>> = stmt.query_map([], row_to_run)?.collect();
        let runs = all?;
        Ok(runs
            .into_iter()
            .filter(|r| {
                filter.status.as_ref().is_none_or(|s| r.status == *s)
                    && filter.vendor.as_ref().is_none_or(|v| r.vendor == *v)
            })
            .collect())
    }
}
