use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredLog {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub component: String,
    pub message: String,
    pub plugin_id: Option<String>,
    pub plugin_version: Option<String>,
    pub platform_id: Option<String>,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub container_id: Option<String>,
    pub error_code: Option<String>,
    pub context: serde_json::Value,
}
