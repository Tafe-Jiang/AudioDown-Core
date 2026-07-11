use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginType;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::StorageError;

#[derive(Debug, Clone)]
pub struct PluginRecord {
    pub plugin_id: PluginId,
    pub plugin_type: PluginType,
    pub platform_id: String,
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    pub source_kind: String,
    pub source_ref: String,
    pub commit_sha: Option<String>,
    pub repository_id: Option<String>,
    pub manifest_json: serde_json::Value,
    pub manifest_hash: String,
    pub source_hash: Option<String>,
    pub image_id: Option<String>,
    pub status: PluginStatus,
    pub run_mode: RunMode,
    pub priority: i64,
    pub enabled: bool,
    pub last_error: Option<String>,
    pub install_operation_id: Option<Uuid>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub installed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct PluginRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> PluginRepository<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert(&self, record: &PluginRecord) -> Result<(), StorageError> {
        validate_priority(record.priority)?;
        sqlx::query(
            r#"
            INSERT INTO plugins (
              plugin_id, plugin_type, platform_id, name, version, protocol_version,
              source_kind, source_ref, commit_sha, repository_id, manifest_json,
              manifest_hash, source_hash, image_id, status, run_mode, priority,
              enabled, last_error, install_operation_id, last_used_at, installed_at,
              updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(plugin_id) DO UPDATE SET
              plugin_type = excluded.plugin_type,
              platform_id = excluded.platform_id,
              name = excluded.name,
              version = excluded.version,
              protocol_version = excluded.protocol_version,
              source_kind = excluded.source_kind,
              source_ref = excluded.source_ref,
              commit_sha = excluded.commit_sha,
              repository_id = excluded.repository_id,
              manifest_json = excluded.manifest_json,
              manifest_hash = excluded.manifest_hash,
              source_hash = excluded.source_hash,
              image_id = excluded.image_id,
              status = excluded.status,
              run_mode = excluded.run_mode,
              priority = excluded.priority,
              enabled = excluded.enabled,
              last_error = excluded.last_error,
              install_operation_id = excluded.install_operation_id,
              last_used_at = excluded.last_used_at,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(record.plugin_id.as_str())
        .bind(plugin_type_to_str(record.plugin_type))
        .bind(&record.platform_id)
        .bind(&record.name)
        .bind(&record.version)
        .bind(&record.protocol_version)
        .bind(&record.source_kind)
        .bind(&record.source_ref)
        .bind(&record.commit_sha)
        .bind(&record.repository_id)
        .bind(serde_json::to_string(&record.manifest_json).map_err(invalid_data)?)
        .bind(&record.manifest_hash)
        .bind(&record.source_hash)
        .bind(&record.image_id)
        .bind(plugin_status_to_str(record.status))
        .bind(run_mode_to_str(record.run_mode))
        .bind(record.priority)
        .bind(record.enabled)
        .bind(&record.last_error)
        .bind(record.install_operation_id.map(|id| id.to_string()))
        .bind(record.last_used_at.map(|timestamp| timestamp.to_rfc3339()))
        .bind(record.installed_at.to_rfc3339())
        .bind(record.updated_at.to_rfc3339())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_installing(&self, record: &PluginRecord) -> Result<(), StorageError> {
        validate_priority(record.priority)?;
        if record.status != PluginStatus::Installing || record.install_operation_id.is_none() {
            return Err(StorageError::InvalidData(
                "installing plugin record requires an operation ID".to_string(),
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO plugins (
              plugin_id, plugin_type, platform_id, name, version, protocol_version,
              source_kind, source_ref, commit_sha, repository_id, manifest_json,
              manifest_hash, source_hash, image_id, status, run_mode, priority,
              enabled, last_error, install_operation_id, last_used_at, installed_at,
              updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(record.plugin_id.as_str())
        .bind(plugin_type_to_str(record.plugin_type))
        .bind(&record.platform_id)
        .bind(&record.name)
        .bind(&record.version)
        .bind(&record.protocol_version)
        .bind(&record.source_kind)
        .bind(&record.source_ref)
        .bind(&record.commit_sha)
        .bind(&record.repository_id)
        .bind(serde_json::to_string(&record.manifest_json).map_err(invalid_data)?)
        .bind(&record.manifest_hash)
        .bind(&record.source_hash)
        .bind(&record.image_id)
        .bind(plugin_status_to_str(record.status))
        .bind(run_mode_to_str(record.run_mode))
        .bind(record.priority)
        .bind(record.enabled)
        .bind(&record.last_error)
        .bind(record.install_operation_id.map(|id| id.to_string()))
        .bind(record.last_used_at.map(|timestamp| timestamp.to_rfc3339()))
        .bind(record.installed_at.to_rfc3339())
        .bind(record.updated_at.to_rfc3339())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_install_result(
        &self,
        plugin_id: &PluginId,
        repository_id: &str,
        image_id: &str,
        source_hash: &str,
        operation_id: Uuid,
        status: PluginStatus,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            r#"
            UPDATE plugins
            SET repository_id = ?, image_id = ?, source_hash = ?,
                install_operation_id = ?, status = ?, updated_at = ?
            WHERE plugin_id = ?
            "#,
        )
        .bind(repository_id)
        .bind(image_id)
        .bind(source_hash)
        .bind(operation_id.to_string())
        .bind(plugin_status_to_str(status))
        .bind(Utc::now().to_rfc3339())
        .bind(plugin_id.as_str())
        .execute(self.pool)
        .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn update_settings(
        &self,
        plugin_id: &PluginId,
        enabled: bool,
        run_mode: RunMode,
        priority: i64,
    ) -> Result<(), StorageError> {
        validate_priority(priority)?;
        let result = sqlx::query(
            r#"
            UPDATE plugins
            SET enabled = ?, run_mode = ?, priority = ?, updated_at = ?
            WHERE plugin_id = ?
            "#,
        )
        .bind(enabled)
        .bind(run_mode_to_str(run_mode))
        .bind(priority)
        .bind(Utc::now().to_rfc3339())
        .bind(plugin_id.as_str())
        .execute(self.pool)
        .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn touch(
        &self,
        plugin_id: &PluginId,
        last_used_at: DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let result =
            sqlx::query("UPDATE plugins SET last_used_at = ?, updated_at = ? WHERE plugin_id = ?")
                .bind(last_used_at.to_rfc3339())
                .bind(Utc::now().to_rfc3339())
                .bind(plugin_id.as_str())
                .execute(self.pool)
                .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn delete(&self, plugin_id: &PluginId) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM plugins WHERE plugin_id = ?")
            .bind(plugin_id.as_str())
            .execute(self.pool)
            .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn list_pending_install_operations(&self) -> Result<Vec<PluginRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT *
            FROM plugins
            WHERE status = 'installing' AND install_operation_id IS NOT NULL
            ORDER BY plugin_id ASC
            "#,
        )
        .fetch_all(self.pool)
        .await?;
        rows.into_iter().map(decode_plugin).collect()
    }

    pub async fn complete_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            r#"
            UPDATE plugins
            SET status = 'installed', install_operation_id = NULL, updated_at = ?
            WHERE plugin_id = ? AND install_operation_id = ?
            "#,
        )
        .bind(Utc::now().to_rfc3339())
        .bind(plugin_id.as_str())
        .bind(operation_id.to_string())
        .execute(self.pool)
        .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn rollback_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<(), StorageError> {
        let result =
            sqlx::query("DELETE FROM plugins WHERE plugin_id = ? AND install_operation_id = ?")
                .bind(plugin_id.as_str())
                .bind(operation_id.to_string())
                .execute(self.pool)
                .await?;
        ensure_one_row(result.rows_affected())
    }

    pub async fn set_status(
        &self,
        plugin_id: &PluginId,
        status: PluginStatus,
    ) -> Result<(), StorageError> {
        sqlx::query("UPDATE plugins SET status = ?, updated_at = ? WHERE plugin_id = ?")
            .bind(plugin_status_to_str(status))
            .bind(Utc::now().to_rfc3339())
            .bind(plugin_id.as_str())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn get(&self, plugin_id: &PluginId) -> Result<Option<PluginRecord>, StorageError> {
        let row = sqlx::query("SELECT * FROM plugins WHERE plugin_id = ?")
            .bind(plugin_id.as_str())
            .fetch_optional(self.pool)
            .await?;
        row.map(decode_plugin).transpose()
    }

    pub async fn list(&self) -> Result<Vec<PluginRecord>, StorageError> {
        let rows = sqlx::query("SELECT * FROM plugins ORDER BY priority ASC, plugin_id ASC")
            .fetch_all(self.pool)
            .await?;
        rows.into_iter().map(decode_plugin).collect()
    }
}

fn decode_plugin(row: sqlx::sqlite::SqliteRow) -> Result<PluginRecord, StorageError> {
    let plugin_id_text: String = row.try_get("plugin_id")?;
    let plugin_id = PluginId::parse(plugin_id_text).map_err(invalid_data)?;
    let plugin_type = plugin_type_from_str(row.try_get("plugin_type")?)?;
    let status = plugin_status_from_str(row.try_get("status")?)?;
    let run_mode = run_mode_from_str(row.try_get("run_mode")?)?;
    let manifest_text: String = row.try_get("manifest_json")?;

    Ok(PluginRecord {
        plugin_id,
        plugin_type,
        platform_id: row.try_get("platform_id")?,
        name: row.try_get("name")?,
        version: row.try_get("version")?,
        protocol_version: row.try_get("protocol_version")?,
        source_kind: row.try_get("source_kind")?,
        source_ref: row.try_get("source_ref")?,
        commit_sha: row.try_get("commit_sha")?,
        repository_id: row.try_get("repository_id")?,
        manifest_json: serde_json::from_str(&manifest_text).map_err(invalid_data)?,
        manifest_hash: row.try_get("manifest_hash")?,
        source_hash: row.try_get("source_hash")?,
        image_id: row.try_get("image_id")?,
        status,
        run_mode,
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
        last_error: row.try_get("last_error")?,
        install_operation_id: parse_optional_uuid(row.try_get("install_operation_id")?)?,
        last_used_at: parse_optional_timestamp(row.try_get("last_used_at")?)?,
        installed_at: parse_timestamp(row.try_get("installed_at")?)?,
        updated_at: parse_timestamp(row.try_get("updated_at")?)?,
    })
}

fn plugin_type_to_str(value: PluginType) -> &'static str {
    match value {
        PluginType::Content => "content",
        PluginType::Credential => "credential",
    }
}

fn plugin_type_from_str(value: String) -> Result<PluginType, StorageError> {
    match value.as_str() {
        "content" => Ok(PluginType::Content),
        "credential" => Ok(PluginType::Credential),
        _ => Err(StorageError::InvalidData(format!(
            "unknown plugin type: {value}"
        ))),
    }
}

fn plugin_status_to_str(value: PluginStatus) -> &'static str {
    match value {
        PluginStatus::Installing => "installing",
        PluginStatus::Installed => "installed",
        PluginStatus::Starting => "starting",
        PluginStatus::Healthy => "healthy",
        PluginStatus::Stopped => "stopped",
        PluginStatus::Unhealthy => "unhealthy",
        PluginStatus::Disabled => "disabled",
    }
}

fn plugin_status_from_str(value: String) -> Result<PluginStatus, StorageError> {
    match value.as_str() {
        "installing" => Ok(PluginStatus::Installing),
        "installed" => Ok(PluginStatus::Installed),
        "starting" => Ok(PluginStatus::Starting),
        "healthy" => Ok(PluginStatus::Healthy),
        "stopped" => Ok(PluginStatus::Stopped),
        "unhealthy" => Ok(PluginStatus::Unhealthy),
        "disabled" => Ok(PluginStatus::Disabled),
        _ => Err(StorageError::InvalidData(format!(
            "unknown plugin status: {value}"
        ))),
    }
}

fn run_mode_to_str(value: RunMode) -> &'static str {
    match value {
        RunMode::OnDemand => "on_demand",
        RunMode::Always => "always",
    }
}

fn run_mode_from_str(value: String) -> Result<RunMode, StorageError> {
    match value.as_str() {
        "on_demand" => Ok(RunMode::OnDemand),
        "always" => Ok(RunMode::Always),
        _ => Err(StorageError::InvalidData(format!(
            "unknown run mode: {value}"
        ))),
    }
}

fn parse_timestamp(value: String) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(&value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(invalid_data)
}

fn parse_optional_timestamp(value: Option<String>) -> Result<Option<DateTime<Utc>>, StorageError> {
    value.map(parse_timestamp).transpose()
}

fn parse_optional_uuid(value: Option<String>) -> Result<Option<Uuid>, StorageError> {
    value
        .map(|id| Uuid::parse_str(&id).map_err(invalid_data))
        .transpose()
}

fn validate_priority(priority: i64) -> Result<(), StorageError> {
    if !(0..=1000).contains(&priority) {
        return Err(StorageError::InvalidData(
            "plugin priority must be between 0 and 1000".to_string(),
        ));
    }
    Ok(())
}

fn ensure_one_row(rows_affected: u64) -> Result<(), StorageError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(StorageError::NotFound)
    }
}

fn invalid_data(error: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidData(error.to_string())
}
