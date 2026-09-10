#![deny(unsafe_code)]

use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};
use serde::Serialize;

use crate::error::StoreError;

use super::Store;

/// One delivery attempt, as written by the notifier after each channel send.
pub struct NewNotifyLog<'a> {
    pub ts: DateTime<Utc>,
    pub run_id: Option<&'a str>,
    pub kind: &'a str,
    pub dedupe_key: &'a str,
    pub channel: &'a str,
    pub ok: bool,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub error: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotifyLogRow {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub run_id: Option<String>,
    pub kind: String,
    pub dedupe_key: String,
    pub channel: String,
    pub ok: bool,
    pub title: String,
    pub body: Option<String>,
    pub error: Option<String>,
}

const COLS: &str = "id, ts, run_id, kind, dedupe_key, channel, ok, title, body, error";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotifyLogRow> {
    Ok(NotifyLogRow {
        id: row.get(0)?,
        ts: row.get(1)?,
        run_id: row.get(2)?,
        kind: row.get(3)?,
        dedupe_key: row.get(4)?,
        channel: row.get(5)?,
        ok: row.get::<_, i64>(6)? != 0,
        title: row.get(7)?,
        body: row.get(8)?,
        error: row.get(9)?,
    })
}

impl Store {
    pub fn append_notify_log(&mut self, entry: NewNotifyLog<'_>) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO notify_log \
             (ts, run_id, kind, dedupe_key, channel, ok, title, body, error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.ts,
                entry.run_id,
                entry.kind,
                entry.dedupe_key,
                entry.channel,
                entry.ok as i64,
                entry.title,
                entry.body,
                entry.error,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Timestamp of the most recent *successful* delivery for `dedupe_key`,
    /// on any channel. `None` when it has never been delivered.
    pub fn last_notify_sent(&self, dedupe_key: &str) -> Result<Option<DateTime<Utc>>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT MAX(ts) FROM notify_log WHERE dedupe_key = ?1 AND ok = 1")?;
        let ts: Option<DateTime<Utc>> = stmt.query_row(params![dedupe_key], |r| r.get(0))?;
        Ok(ts)
    }

    /// Most recent `limit` delivery attempts, newest first. `run_id` narrows
    /// to one run; `None` returns everything including global entries.
    pub fn list_notify_log(
        &self,
        run_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NotifyLogRow>, StoreError> {
        let limit = limit as i64;
        let rows: rusqlite::Result<Vec<NotifyLogRow>> = match run_id {
            Some(rid) => {
                let sql = format!(
                    "SELECT {COLS} FROM notify_log WHERE run_id = ?1 \
                     ORDER BY ts DESC, id DESC LIMIT ?2"
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let it = stmt.query_map(params![rid, limit], map_row)?;
                it.collect()
            }
            None => {
                let sql =
                    format!("SELECT {COLS} FROM notify_log ORDER BY ts DESC, id DESC LIMIT ?1");
                let mut stmt = self.conn.prepare(&sql)?;
                let it = stmt.query_map(params![limit], map_row)?;
                it.collect()
            }
        };
        Ok(rows?)
    }
}
