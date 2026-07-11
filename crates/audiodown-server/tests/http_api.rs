use std::sync::Arc;

use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_plugin_manager::service::PluginManagerService;
use audiodown_server::{
    app::build_router,
    plugin_manager_adapters::{
        ConfiguredLifecycleRiskAuthorizer, SqlitePluginManagerStore, SupervisorPluginRuntime,
        UnavailableRepositorySource,
    },
    state::{AppState, UnavailableSupervisorClient},
};
use audiodown_storage::{PluginRecord, Storage};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use chrono::Utc;
use semver::Version;
use tower::ServiceExt;

async fn test_app(with_plugin: bool) -> axum::Router {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    storage.migrate().await.unwrap();
    if with_plugin {
        storage.plugins().upsert(&plugin_record()).await.unwrap();
    }
    let supervisor = Arc::new(UnavailableSupervisorClient);
    let manager = Arc::new(
        PluginManagerService::new(
            Arc::new(SqlitePluginManagerStore::new(storage.clone())),
            Arc::new(UnavailableRepositorySource),
            std::env::temp_dir().join("audiodown-http-api-manager"),
            Version::parse("1.0.0-alpha.1").unwrap(),
            Version::new(1, 0, 0),
        )
        .with_installation_ports(
            Arc::new(SupervisorPluginRuntime::new(supervisor.clone())),
            Arc::new(ConfiguredLifecycleRiskAuthorizer::new(Default::default())),
        ),
    );
    let state = AppState::new(
        storage,
        Version::parse("1.0.0-alpha.1").unwrap(),
        supervisor,
    )
    .with_plugin_manager(manager);
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
    let app = test_app(false).await;

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
        assert_eq!(empty_state["emptyState"]["reason"], "NO_CONTENT_PLUGINS");
        let serialized = empty_state.to_string().to_ascii_lowercase();
        assert!(!serialized.contains("hardcoded-platform-label"));
    }
}

#[tokio::test]
async fn returns_stable_error_when_supervisor_is_unavailable() {
    let app = test_app(true).await;
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

fn plugin_record() -> PluginRecord {
    let now = Utc::now();
    PluginRecord {
        plugin_id: PluginId::parse("com.audiodown.virtual.content").unwrap(),
        plugin_type: PluginType::Content,
        platform_id: "virtual".to_string(),
        name: "Virtual Content".to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_kind: "fixture".to_string(),
        source_ref: "virtual-contract-fixture".to_string(),
        commit_sha: None,
        repository_id: None,
        manifest_json: serde_json::json!({}),
        manifest_hash: "a".repeat(64),
        source_hash: None,
        image_id: Some("audiodown/plugin-virtual:dev".to_string()),
        status: PluginStatus::Installed,
        run_mode: RunMode::OnDemand,
        priority: 100,
        enabled: true,
        last_error: None,
        install_operation_id: None,
        last_used_at: None,
        installed_at: now,
        updated_at: now,
    }
}
