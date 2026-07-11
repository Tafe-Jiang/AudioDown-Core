use audiodown_domain::plugin::{PluginId, PluginStatus};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    routes::{internal_error, ApiError, ApiResult},
    state::AppState,
    supervisor::{PluginRuntimeState, SupervisorError},
};

#[derive(Debug, Serialize)]
pub struct PluginListResponse {
    pub items: Vec<PluginItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginItem {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub status: PluginStatus,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyStateResponse {
    pub reason: &'static str,
    pub title: &'static str,
    pub action_label: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

pub async fn list(State(state): State<AppState>) -> ApiResult<PluginListResponse> {
    let records = state
        .storage
        .plugins()
        .list()
        .await
        .map_err(internal_error)?;
    let items = records
        .into_iter()
        .map(|record| PluginItem {
            plugin_id: record.plugin_id.to_string(),
            name: record.name,
            version: record.version,
            status: record.status,
            enabled: record.enabled,
        })
        .collect();
    Ok(Json(PluginListResponse { items }))
}

pub async fn discover() -> Json<EmptyStateResponse> {
    Json(empty_state())
}

pub async fn search(Query(query): Query<SearchQuery>) -> Json<EmptyStateResponse> {
    let _ = query.q;
    Json(empty_state())
}

pub async fn start(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let runtime = state
        .supervisor
        .start_plugin(&plugin_id)
        .await
        .map_err(supervisor_error)?;
    Ok(Json(runtime))
}

pub async fn stop(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let runtime = state
        .supervisor
        .stop_plugin(&plugin_id)
        .await
        .map_err(supervisor_error)?;
    Ok(Json(runtime))
}

fn empty_state() -> EmptyStateResponse {
    EmptyStateResponse {
        reason: "NO_CONTENT_PLUGINS",
        title: "尚未安装内容插件",
        action_label: "添加 GitHub 插件仓库",
    }
}

fn parse_plugin_id(
    plugin_id: String,
) -> Result<PluginId, (StatusCode, Json<ApiError>)> {
    PluginId::parse(plugin_id).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                code: "INVALID_PLUGIN_ID",
                message: error.to_string(),
            }),
        )
    })
}

fn supervisor_error(error: SupervisorError) -> (StatusCode, Json<ApiError>) {
    if error.is_unavailable() {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                code: "SUPERVISOR_UNAVAILABLE",
                message: "Plugin management service is unavailable".to_string(),
            }),
        )
    } else {
        tracing::warn!(error = %error, "Supervisor rejected plugin lifecycle request");
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                code: "SUPERVISOR_ERROR",
                message: "Plugin lifecycle request failed".to_string(),
            }),
        )
    }
}
