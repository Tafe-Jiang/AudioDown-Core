use axum::{
    routing::{delete, get, patch, post},
    Router,
};

use crate::{
    routes::{health, logs, plugins, repositories, system},
    state::AppState,
    web,
};

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/system", get(system::system))
        .route("/plugins", get(plugins::list))
        .route("/plugin-repositories/inspect", post(repositories::inspect))
        .route(
            "/plugin-repositories/{snapshot_id}/plugins/{plugin_id}/install",
            post(plugins::install),
        )
        .route("/plugins/{plugin_id}/start", post(plugins::start))
        .route("/plugins/{plugin_id}/stop", post(plugins::stop))
        .route("/plugins/{plugin_id}/runtime", get(plugins::runtime))
        .route("/plugins/{plugin_id}", patch(plugins::update))
        .route("/plugins/{plugin_id}", delete(plugins::uninstall))
        .route(
            "/dev/plugins/register-fixture",
            post(plugins::register_fixture),
        )
        .route("/logs", get(logs::list))
        .route("/discover", get(plugins::discover))
        .route("/search", get(plugins::search))
        .fallback(crate::routes::not_found);

    Router::new()
        .route("/healthz", get(health::health))
        .nest("/api/v1", api)
        .fallback(web::serve)
        .with_state(state)
}
