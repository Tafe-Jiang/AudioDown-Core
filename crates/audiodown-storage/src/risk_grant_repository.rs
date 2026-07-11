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

    pub async fn replace_commit_grant(&self, record: &RiskGrantRecord) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO plugin_risk_grants (
              id, repository_id, plugin_id, commit_sha, risk_kind, reason, granted_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(plugin_id, commit_sha, risk_kind) DO UPDATE SET
              id = excluded.id,
              repository_id = excluded.repository_id,
              reason = excluded.reason,
              granted_at = excluded.granted_at
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

    pub async fn get_for(
        &self,
        plugin_id: &PluginId,
        commit_sha: &str,
        risk_kind: &str,
    ) -> Result<Option<RiskGrantRecord>, StorageError> {
        let row = sqlx::query(
            r#"
            SELECT id, repository_id, plugin_id, commit_sha, risk_kind, reason, granted_at
            FROM plugin_risk_grants
            WHERE plugin_id = ? AND commit_sha = ? AND risk_kind = ?
            "#,
        )
        .bind(plugin_id.as_str())
        .bind(commit_sha)
        .bind(risk_kind)
        .fetch_optional(self.pool)
        .await?;
        row.map(|row| {
            use sqlx::Row;

            let id: String = row.try_get("id")?;
            let plugin_id: String = row.try_get("plugin_id")?;
            let granted_at: String = row.try_get("granted_at")?;
            Ok(RiskGrantRecord {
                id: Uuid::parse_str(&id).map_err(invalid_data)?,
                repository_id: row.try_get("repository_id")?,
                plugin_id: PluginId::parse(plugin_id).map_err(invalid_data)?,
                commit_sha: row.try_get("commit_sha")?,
                risk_kind: row.try_get("risk_kind")?,
                reason: row.try_get("reason")?,
                granted_at: DateTime::parse_from_rfc3339(&granted_at)
                    .map_err(invalid_data)?
                    .with_timezone(&Utc),
            })
        })
        .transpose()
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

fn invalid_data(error: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidData(error.to_string())
}
