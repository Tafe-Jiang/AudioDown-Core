use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};

use crate::{
    routes::{content, health, logs, plugins, repositories, system},
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
        .route("/search", get(content::search))
        .route("/discover", get(content::discover))
        .route("/categories", get(content::categories))
        .route("/albums/get", post(content::album_get))
        .route("/tracks/list", post(content::tracks_list))
        .route(
            "/plugins/{plugin_id}/content-settings",
            patch(content::update_settings),
        )
        .route(
            "/platforms/{platform_id}/default-content-plugin",
            put(content::set_default),
        )
        .fallback(crate::routes::not_found);

    Router::new()
        .route("/healthz", get(health::health))
        .nest("/api/v1", api)
        .fallback(web::serve)
        .with_state(state)
}
