use audiodown_domain::plugin::PluginId;
use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::StorageError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentParticipation {
    pub search_enabled: bool,
    pub discover_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentParticipationKind {
    Search,
    Discover,
}

#[derive(Debug, Clone)]
pub struct ContentRoutingCandidate {
    pub plugin_id: PluginId,
    pub platform_id: String,
    pub name: String,
    pub version: String,
    pub priority: i64,
    pub is_default: bool,
    pub manifest_json: serde_json::Value,
}

pub struct ContentRoutingRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ContentRoutingRepository<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn participation(
        &self,
        plugin_id: &PluginId,
    ) -> Result<ContentParticipation, StorageError> {
        let row = sqlx::query(
            r#"
            SELECT plugin_type, search_enabled, discover_enabled
            FROM plugins
            WHERE plugin_id = ?
            "#,
        )
        .bind(plugin_id.as_str())
        .fetch_optional(self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        ensure_content_type(row.try_get("plugin_type")?)?;
        Ok(ContentParticipation {
            search_enabled: row.try_get("search_enabled")?,
            discover_enabled: row.try_get("discover_enabled")?,
        })
    }

    pub async fn update_participation(
        &self,
        plugin_id: &PluginId,
        participation: ContentParticipation,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let plugin_type: Option<String> =
            sqlx::query_scalar("SELECT plugin_type FROM plugins WHERE plugin_id = ?")
                .bind(plugin_id.as_str())
                .fetch_optional(&mut *transaction)
                .await?;
        ensure_content_type(plugin_type.ok_or(StorageError::NotFound)?)?;
        let result = sqlx::query(
            r#"
            UPDATE plugins
            SET search_enabled = ?, discover_enabled = ?, updated_at = ?
            WHERE plugin_id = ?
            "#,
        )
        .bind(participation.search_enabled)
        .bind(participation.discover_enabled)
        .bind(Utc::now().to_rfc3339())
        .bind(plugin_id.as_str())
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::NotFound);
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_default(
        &self,
        platform_id: &str,
        plugin_id: &PluginId,
    ) -> Result<(), StorageError> {
        validate_platform_id(platform_id)?;
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query("SELECT plugin_type, platform_id FROM plugins WHERE plugin_id = ?")
            .bind(plugin_id.as_str())
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::NotFound)?;
        ensure_content_type(row.try_get("plugin_type")?)?;
        let stored_platform: String = row.try_get("platform_id")?;
        if stored_platform != platform_id {
            return Err(StorageError::InvalidData(
                "default content plugin must belong to the requested platform".to_string(),
            ));
        }

        sqlx::query(
            r#"
            INSERT INTO platform_content_defaults (platform_id, plugin_id, updated_at)
            VALUES (?, ?, ?)
            ON CONFLICT(platform_id) DO UPDATE SET
              plugin_id = excluded.plugin_id,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(platform_id)
        .bind(plugin_id.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn default_for_platform(
        &self,
        platform_id: &str,
    ) -> Result<Option<PluginId>, StorageError> {
        validate_platform_id(platform_id)?;
        let plugin_id: Option<String> = sqlx::query_scalar(
            "SELECT plugin_id FROM platform_content_defaults WHERE platform_id = ?",
        )
        .bind(platform_id)
        .fetch_optional(self.pool)
        .await?;
        plugin_id
            .map(|plugin_id| PluginId::parse(plugin_id).map_err(invalid_data))
            .transpose()
    }

    pub async fn list_candidates(
        &self,
        participation: ContentParticipationKind,
        platform_id: Option<&str>,
        plugin_id: Option<&PluginId>,
    ) -> Result<Vec<ContentRoutingCandidate>, StorageError> {
        if let Some(platform_id) = platform_id {
            validate_platform_id(platform_id)?;
        }
        let participation_column = match participation {
            ContentParticipationKind::Search => "p.search_enabled",
            ContentParticipationKind::Discover => "p.discover_enabled",
        };
        let query = format!(
            r#"
            SELECT
              p.plugin_id,
              p.platform_id,
              p.name,
              p.version,
              p.priority,
              p.manifest_json,
              CASE WHEN d.plugin_id IS NULL THEN 0 ELSE 1 END AS is_default
            FROM plugins p
            LEFT JOIN platform_content_defaults d
              ON d.platform_id = p.platform_id AND d.plugin_id = p.plugin_id
            WHERE p.plugin_type = 'content'
              AND p.enabled = 1
              AND p.status IN ('installed', 'healthy', 'stopped')
              AND {participation_column} = 1
            ORDER BY
              p.platform_id ASC,
              is_default DESC,
              p.priority ASC,
              p.plugin_id ASC
            "#
        );
        let rows = sqlx::query(&query).fetch_all(self.pool).await?;
        rows.into_iter()
            .map(decode_candidate)
            .filter(|candidate| match candidate {
                Ok(candidate) => {
                    platform_id.is_none_or(|expected| candidate.platform_id == expected)
                        && plugin_id.is_none_or(|expected| &candidate.plugin_id == expected)
                }
                Err(_) => true,
            })
            .collect()
    }
}

fn decode_candidate(row: sqlx::sqlite::SqliteRow) -> Result<ContentRoutingCandidate, StorageError> {
    let plugin_id: String = row.try_get("plugin_id")?;
    let manifest_json: String = row.try_get("manifest_json")?;
    Ok(ContentRoutingCandidate {
        plugin_id: PluginId::parse(plugin_id).map_err(invalid_data)?,
        platform_id: row.try_get("platform_id")?,
        name: row.try_get("name")?,
        version: row.try_get("version")?,
        priority: row.try_get("priority")?,
        is_default: row.try_get("is_default")?,
        manifest_json: serde_json::from_str(&manifest_json).map_err(invalid_data)?,
    })
}

fn ensure_content_type(plugin_type: String) -> Result<(), StorageError> {
    if plugin_type == "content" {
        Ok(())
    } else {
        Err(StorageError::InvalidData(
            "content routing settings require a content plugin".to_string(),
        ))
    }
}

fn validate_platform_id(platform_id: &str) -> Result<(), StorageError> {
    if platform_id.is_empty()
        || platform_id.len() > 128
        || !platform_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        Err(StorageError::InvalidData(
            "platform ID is invalid".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn invalid_data(error: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidData(error.to_string())
}
