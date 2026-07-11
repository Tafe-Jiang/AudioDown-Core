use audiodown_domain::{log::StructuredLog, plugin::PluginId};
use audiodown_storage::LogFilter;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    routes::{internal_error, ApiError, ApiResult},
    state::AppState,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    pub plugin_id: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct LogListResponse {
    pub items: Vec<StructuredLog>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<LogQuery>,
) -> ApiResult<LogListResponse> {
    let plugin_id = query
        .plugin_id
        .map(PluginId::parse)
        .transpose()
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    code: "INVALID_PLUGIN_ID",
                    message: error.to_string(),
                }),
            )
        })?;
    let items = state
        .storage
        .logs()
        .list(LogFilter {
            plugin_id,
            limit: query.limit.unwrap_or(100),
        })
        .await
        .map_err(internal_error)?;
    Ok(Json(LogListResponse { items }))
}
