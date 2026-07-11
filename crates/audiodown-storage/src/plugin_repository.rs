use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginType;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

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
    pub manifest_json: serde_json::Value,
    pub manifest_hash: String,
    pub image_id: Option<String>,
    pub status: PluginStatus,
    pub run_mode: RunMode,
    pub priority: i64,
    pub enabled: bool,
    pub last_error: Option<String>,
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
        sqlx::query(
            r#"
            INSERT INTO plugins (
              plugin_id, plugin_type, platform_id, name, version, protocol_version,
              source_kind, source_ref, commit_sha, manifest_json, manifest_hash,
              image_id, status, run_mode, priority, enabled, last_error,
              installed_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(plugin_id) DO UPDATE SET
              plugin_type = excluded.plugin_type,
              platform_id = excluded.platform_id,
              name = excluded.name,
              version = excluded.version,
              protocol_version = excluded.protocol_version,
              source_kind = excluded.source_kind,
              source_ref = excluded.source_ref,
              commit_sha = excluded.commit_sha,
              manifest_json = excluded.manifest_json,
              manifest_hash = excluded.manifest_hash,
              image_id = excluded.image_id,
              status = excluded.status,
              run_mode = excluded.run_mode,
              priority = excluded.priority,
              enabled = excluded.enabled,
              last_error = excluded.last_error,
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
        .bind(serde_json::to_string(&record.manifest_json).map_err(invalid_data)?)
        .bind(&record.manifest_hash)
        .bind(&record.image_id)
        .bind(plugin_status_to_str(record.status))
        .bind(run_mode_to_str(record.run_mode))
        .bind(record.priority)
        .bind(record.enabled)
        .bind(&record.last_error)
        .bind(record.installed_at.to_rfc3339())
        .bind(record.updated_at.to_rfc3339())
        .execute(self.pool)
        .await?;
        Ok(())
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
        manifest_json: serde_json::from_str(&manifest_text).map_err(invalid_data)?,
        manifest_hash: row.try_get("manifest_hash")?,
        image_id: row.try_get("image_id")?,
        status,
        run_mode,
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
        last_error: row.try_get("last_error")?,
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

fn invalid_data(error: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidData(error.to_string())
}
