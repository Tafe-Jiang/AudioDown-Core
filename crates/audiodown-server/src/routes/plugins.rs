use audiodown_domain::plugin::PluginStatus;
use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    routes::{internal_error, ApiResult},
    state::AppState,
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

fn empty_state() -> EmptyStateResponse {
    EmptyStateResponse {
        reason: "NO_CONTENT_PLUGINS",
        title: "尚未安装内容插件",
        action_label: "添加 GitHub 插件仓库",
    }
}
