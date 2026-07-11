pub mod health;
pub mod logs;
pub mod plugins;
pub mod repositories;
pub mod system;

use axum::{http::StatusCode, Json};
use serde::Serialize;

pub type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

pub fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<ApiError>) {
    tracing::error!(error = %error, "request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            code: "INTERNAL_ERROR",
            message: "The request could not be completed".to_string(),
        }),
    )
}

pub async fn not_found() -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError {
            code: "NOT_FOUND",
            message: "API route was not found".to_string(),
        }),
    )
}
