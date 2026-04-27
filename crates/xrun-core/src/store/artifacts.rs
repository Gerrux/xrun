#![deny(unsafe_code)]

use rusqlite::{params, TransactionBehavior};

use crate::error::StoreError;

use super::Store;

pub struct NewArtifact {
    pub kind: String,
    pub remote_path: String,
    pub local_path: Option<String>,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    pub is_best: bool,
}

impl Store {
    pub fn record_artifact(
        &mut self,
        run_id: &super::RunId,
        artifact: NewArtifact,
    ) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO artifacts (run_id, kind, remote_path, local_path, size_bytes, sha256, is_best) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run_id,
                artifact.kind,
                artifact.remote_path,
                artifact.local_path,
                artifact.size_bytes,
                artifact.sha256,
                i32::from(artifact.is_best)
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
}
