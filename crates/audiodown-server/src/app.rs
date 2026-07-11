use axum::{
    routing::{get, post},
    Router,
};

use crate::{
    routes::{health, logs, plugins, system},
    state::AppState,
};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::health))
        .route("/api/v1/system", get(system::system))
        .route("/api/v1/plugins", get(plugins::list))
        .route(
            "/api/v1/plugins/{plugin_id}/start",
            post(plugins::start),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/stop",
            post(plugins::stop),
        )
        .route("/api/v1/logs", get(logs::list))
        .route("/api/v1/discover", get(plugins::discover))
        .route("/api/v1/search", get(plugins::search))
        .with_state(state)
}
