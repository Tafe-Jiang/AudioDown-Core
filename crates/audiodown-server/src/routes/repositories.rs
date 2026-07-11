use audiodown_plugin_manager::service::{InspectionError, RepositoryInspection};
use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;

use crate::{
    routes::{ApiError, ApiResult},
    state::AppState,
};

const MAX_REPOSITORY_URL_BYTES: usize = 512;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectRepositoryRequest {
    pub url: String,
}

pub async fn inspect(
    State(state): State<AppState>,
    Json(request): Json<InspectRepositoryRequest>,
) -> ApiResult<RepositoryInspection> {
    if request.url.len() > MAX_REPOSITORY_URL_BYTES {
        return Err(repository_error(InspectionError::InvalidRepositoryUrl));
    }
    state
        .plugin_manager
        .inspect_repository(&request.url)
        .await
        .map(Json)
        .map_err(repository_error)
}

fn repository_error(error: InspectionError) -> (StatusCode, Json<ApiError>) {
    let (status, code, message) = match error {
        InspectionError::InvalidRepositoryUrl => (
            StatusCode::BAD_REQUEST,
            "INVALID_REPOSITORY_URL",
            "The public repository URL is invalid",
        ),
        InspectionError::RepositoryUnavailable => (
            StatusCode::BAD_GATEWAY,
            "REPOSITORY_UNAVAILABLE",
            "The repository service is unavailable",
        ),
        InspectionError::InvalidRepository => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_REPOSITORY",
            "The repository content is invalid",
        ),
        InspectionError::Busy => (
            StatusCode::TOO_MANY_REQUESTS,
            "REPOSITORY_INSPECTION_BUSY",
            "Two repository inspections are already running",
        ),
        InspectionError::Internal => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "The repository inspection could not be completed",
        ),
    };
    (
        status,
        Json(ApiError {
            code,
            message: message.to_string(),
        }),
    )
}
