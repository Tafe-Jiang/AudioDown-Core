use std::sync::Arc;

use audiodown_server::{
    app::build_router,
    state::{AppState, UnavailableSupervisorClient},
};
use audiodown_storage::Storage;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use semver::Version;
use tower::ServiceExt;

async fn test_app() -> axum::Router {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    storage.migrate().await.unwrap();
    let state = AppState::new(
        storage,
        Version::parse("1.0.0-alpha.1").unwrap(),
        Arc::new(UnavailableSupervisorClient),
    );
    build_router(state)
}

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn exposes_foundation_api_surface() {
    let app = test_app().await;

    let (status, health) = get_json(app.clone(), "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        health,
        serde_json::json!({"ok": true, "service": "audiodown-core"})
    );

    let (status, system) = get_json(app.clone(), "/api/v1/system").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(system["version"], "1.0.0-alpha.1");
    assert_eq!(system["supervisor"]["available"], false);
    assert_eq!(system["pluginCount"], 0);

    let (status, plugins) = get_json(app.clone(), "/api/v1/plugins").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plugins, serde_json::json!({"items": []}));

    let (status, logs) = get_json(app.clone(), "/api/v1/logs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(logs, serde_json::json!({"items": []}));

    for uri in ["/api/v1/discover", "/api/v1/search?q=test"] {
        let (status, empty_state) = get_json(app.clone(), uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(empty_state["reason"], "NO_CONTENT_PLUGINS");
        let serialized = empty_state.to_string().to_ascii_lowercase();
        assert!(!serialized.contains("hardcoded-platform-label"));
    }
}

#[tokio::test]
async fn returns_stable_error_when_supervisor_is_unavailable() {
    let app = test_app().await;
    let response = app
        .oneshot(
            Request::post("/api/v1/plugins/com.audiodown.virtual.content/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["code"], "SUPERVISOR_UNAVAILABLE");
}
