use axum::{extract::State, Json};
use serde::Serialize;

use crate::{
    routes::{internal_error, ApiResult},
    state::AppState,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemResponse {
    pub version: String,
    pub supervisor: SupervisorStatus,
    pub plugin_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorStatus {
    pub available: bool,
    pub error: Option<String>,
}

pub async fn system(State(state): State<AppState>) -> ApiResult<SystemResponse> {
    let plugin_count = state
        .storage
        .plugins()
        .list()
        .await
        .map_err(internal_error)?
        .len();
    let supervisor = match state.supervisor.ping().await {
        Ok(_) => SupervisorStatus {
            available: true,
            error: None,
        },
        Err(error) => SupervisorStatus {
            available: false,
            error: Some(error.to_string()),
        },
    };

    Ok(Json(SystemResponse {
        version: state.core_version.to_string(),
        supervisor,
        plugin_count,
    }))
}
