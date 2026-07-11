use audiodown_domain::{
    log::{LogLevel, StructuredLog},
    plugin::PluginId,
};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::StorageError;

#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub plugin_id: Option<PluginId>,
    pub limit: u32,
}

pub struct LogRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> LogRepository<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn append(&self, log: &StructuredLog) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO structured_logs (
              id, timestamp, level, component, message, plugin_id, plugin_version,
              platform_id, request_id, task_id, container_id, error_code, context_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(log.id.to_string())
        .bind(log.timestamp.to_rfc3339())
        .bind(log_level_to_str(&log.level))
        .bind(&log.component)
        .bind(&log.message)
        .bind(&log.plugin_id)
        .bind(&log.plugin_version)
        .bind(&log.platform_id)
        .bind(&log.request_id)
        .bind(&log.task_id)
        .bind(&log.container_id)
        .bind(&log.error_code)
        .bind(serde_json::to_string(&log.context).map_err(invalid_data)?)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn append_if_absent(&self, log: &StructuredLog) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO structured_logs (
              id, timestamp, level, component, message, plugin_id, plugin_version,
              platform_id, request_id, task_id, container_id, error_code, context_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(log.id.to_string())
        .bind(log.timestamp.to_rfc3339())
        .bind(log_level_to_str(&log.level))
        .bind(&log.component)
        .bind(&log.message)
        .bind(&log.plugin_id)
        .bind(&log.plugin_version)
        .bind(&log.platform_id)
        .bind(&log.request_id)
        .bind(&log.task_id)
        .bind(&log.container_id)
        .bind(&log.error_code)
        .bind(serde_json::to_string(&log.context).map_err(invalid_data)?)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list(&self, filter: LogFilter) -> Result<Vec<StructuredLog>, StorageError> {
        let limit = i64::from(filter.limit.clamp(1, 1000));
        let rows = if let Some(plugin_id) = filter.plugin_id {
            sqlx::query(
                "SELECT * FROM structured_logs WHERE plugin_id = ? ORDER BY timestamp DESC LIMIT ?",
            )
            .bind(plugin_id.as_str())
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query("SELECT * FROM structured_logs ORDER BY timestamp DESC LIMIT ?")
                .bind(limit)
                .fetch_all(self.pool)
                .await?
        };

        rows.into_iter().map(decode_log).collect()
    }
}

fn decode_log(row: sqlx::sqlite::SqliteRow) -> Result<StructuredLog, StorageError> {
    let id: String = row.try_get("id")?;
    let timestamp: String = row.try_get("timestamp")?;
    let level: String = row.try_get("level")?;
    let context: String = row.try_get("context_json")?;

    Ok(StructuredLog {
        id: Uuid::parse_str(&id).map_err(invalid_data)?,
        timestamp: parse_timestamp(timestamp)?,
        level: log_level_from_str(level)?,
        component: row.try_get("component")?,
        message: row.try_get("message")?,
        plugin_id: row.try_get("plugin_id")?,
        plugin_version: row.try_get("plugin_version")?,
        platform_id: row.try_get("platform_id")?,
        request_id: row.try_get("request_id")?,
        task_id: row.try_get("task_id")?,
        container_id: row.try_get("container_id")?,
        error_code: row.try_get("error_code")?,
        context: serde_json::from_str(&context).map_err(invalid_data)?,
    })
}

fn log_level_to_str(level: &LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

fn log_level_from_str(value: String) -> Result<LogLevel, StorageError> {
    match value.as_str() {
        "trace" => Ok(LogLevel::Trace),
        "debug" => Ok(LogLevel::Debug),
        "info" => Ok(LogLevel::Info),
        "warn" => Ok(LogLevel::Warn),
        "error" => Ok(LogLevel::Error),
        _ => Err(StorageError::InvalidData(format!(
            "unknown log level: {value}"
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
