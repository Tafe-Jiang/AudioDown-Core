use std::sync::Arc;

use audiodown_server::{
    app::build_router,
    state::{AppState, UnavailableSupervisorClient},
};
use audiodown_storage::Storage;
use axum::{
    body::{to_bytes, Body},
    http::{header::CONTENT_TYPE, Request, StatusCode},
};
use semver::Version;
use tower::ServiceExt;

async fn test_app() -> axum::Router {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    storage.migrate().await.unwrap();
    build_router(AppState::new(
        storage,
        Version::parse("1.0.0-alpha.1").unwrap(),
        Arc::new(UnavailableSupervisorClient),
    ))
}

async fn get(app: axum::Router, uri: &str) -> axum::response::Response {
    app.oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn serves_embedded_index_and_spa_fallback() {
    let app = test_app().await;

    for uri in ["/", "/discover/deep-link"] {
        let response = get(app.clone(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(String::from_utf8(body.to_vec())
            .unwrap()
            .contains("AudioDown 1.0"));
    }
}

#[tokio::test]
async fn keeps_unknown_api_routes_as_json_not_found() {
    let response = get(test_app().await, "/api/v1/not-found").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.headers()[CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("application/json"));
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["code"], "NOT_FOUND");
}
