use audiodown_domain::plugin::PluginId;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::StorageError;

#[derive(Debug, Clone)]
pub struct RiskGrantRecord {
    pub id: Uuid,
    pub repository_id: String,
    pub plugin_id: PluginId,
    pub commit_sha: String,
    pub risk_kind: String,
    pub reason: String,
    pub granted_at: DateTime<Utc>,
}

pub struct RiskGrantRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> RiskGrantRepository<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, record: &RiskGrantRecord) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO plugin_risk_grants (
              id, repository_id, plugin_id, commit_sha, risk_kind, reason, granted_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(record.id.to_string())
        .bind(&record.repository_id)
        .bind(record.plugin_id.as_str())
        .bind(&record.commit_sha)
        .bind(&record.risk_kind)
        .bind(&record.reason)
        .bind(record.granted_at.to_rfc3339())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn exists_for(
        &self,
        plugin_id: &PluginId,
        commit_sha: &str,
        risk_kind: &str,
    ) -> Result<bool, StorageError> {
        let exists: i64 = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM plugin_risk_grants
              WHERE plugin_id = ? AND commit_sha = ? AND risk_kind = ?
            )
            "#,
        )
        .bind(plugin_id.as_str())
        .bind(commit_sha)
        .bind(risk_kind)
        .fetch_one(self.pool)
        .await?;
        Ok(exists != 0)
    }
}
