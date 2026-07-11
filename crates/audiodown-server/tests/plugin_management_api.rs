use std::sync::Arc;

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_plugin_manager::{
    service::{PluginManagerService, PluginRuntimeControl},
    PluginManagerError,
};
use audiodown_server::{
    app::build_router,
    plugin_manager_adapters::{SqlitePluginManagerStore, UnavailableRepositorySource},
    state::{AppState, UnavailableSupervisorClient},
};
use audiodown_storage::{PluginRecord, Storage};
use audiodown_supervisor_protocol::{
    PluginInstallOperation, PluginInstallOperationList, PluginRemoveResult, PluginRuntimeState,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use chrono::Utc;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn exposes_expanded_plugin_items_and_manager_owned_lifecycle_routes() {
    let fixture = ApiFixture::new(false).await;

    let response = fixture
        .request(
            Request::post(format!("/api/v1/plugins/{}/start", fixture.plugin_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await["status"], "healthy");

    let response = fixture
        .request(
            Request::get(format!("/api/v1/plugins/{}/runtime", fixture.plugin_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await["status"], "healthy");

    let response = fixture
        .request(Request::get("/api/v1/plugins").body(Body::empty()).unwrap())
        .await;
    let item = &json(response).await["items"][0];
    assert_eq!(item["pluginId"], fixture.plugin_id.as_str());
    assert_eq!(item["pluginType"], "content");
    assert_eq!(item["platformId"], "virtual");
    assert_eq!(item["runMode"], "on_demand");
    assert_eq!(item["priority"], 100);
    assert_eq!(
        item["sourceUrl"],
        "https://github.com/example-owner/example-repository"
    );
    assert_eq!(
        item["commitSha"],
        "0123456789abcdef0123456789abcdef01234567"
    );
}

#[tokio::test]
async fn patches_settings_and_rejects_invalid_inputs() {
    let fixture = ApiFixture::new(false).await;
    let response = fixture
        .request(
            Request::patch(format!("/api/v1/plugins/{}", fixture.plugin_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"enabled":true,"runMode":"always","priority":25}"#,
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let item = json(response).await;
    assert_eq!(item["runMode"], "always");
    assert_eq!(item["priority"], 25);
    assert_eq!(item["status"], "healthy");

    let response = fixture
        .request(
            Request::patch(format!("/api/v1/plugins/{}", fixture.plugin_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"enabled":true,"runMode":"on_demand","priority":1001}"#,
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["code"], "INVALID_PRIORITY");

    let response = fixture
        .request(
            Request::patch(format!("/api/v1/plugins/{}", fixture.plugin_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"enabled":true,"runMode":"sometimes","priority":10}"#,
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn uninstall_deletes_only_after_runtime_cleanup() {
    let fixture = ApiFixture::new(false).await;
    let response = fixture
        .request(
            Request::delete(format!("/api/v1/plugins/{}", fixture.plugin_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(fixture
        .storage
        .plugins()
        .get(&fixture.plugin_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn runtime_failure_preserves_settings_and_maps_stable_errors() {
    let fixture = ApiFixture::new(true).await;
    let response = fixture
        .request(
            Request::patch(format!("/api/v1/plugins/{}", fixture.plugin_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"enabled":true,"runMode":"always","priority":25}"#,
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(response).await["code"], "SUPERVISOR_UNAVAILABLE");
    let record = fixture
        .storage
        .plugins()
        .get(&fixture.plugin_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.run_mode, RunMode::OnDemand);
    assert_eq!(record.priority, 100);
    assert_eq!(
        record.last_error.as_deref(),
        Some("plugin runtime action failed")
    );
}

#[tokio::test]
async fn management_routes_are_thin_manager_adapters() {
    let source = include_str!("../src/routes/plugins.rs");
    for (start, end) in [
        ("pub async fn start(", "pub async fn stop("),
        ("pub async fn stop(", "pub async fn runtime("),
        ("pub async fn runtime(", "#[derive(Debug, Deserialize)]"),
        ("pub async fn update(", "pub async fn uninstall("),
        ("pub async fn uninstall(", "fn token_matches("),
    ] {
        let body = section(source, start, end);
        assert!(!body.contains(".storage"));
        assert!(!body.contains(".supervisor"));
        assert!(body.contains(".plugin_manager"));
    }
}

struct ApiFixture {
    _temp: TempDir,
    app: axum::Router,
    storage: Storage,
    plugin_id: PluginId,
}

impl ApiFixture {
    async fn new(fail_runtime: bool) -> Self {
        let temp = TempDir::new().unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        storage.migrate().await.unwrap();
        let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
        storage
            .plugins()
            .upsert(&plugin_record(plugin_id.clone()))
            .await
            .unwrap();
        let manager = Arc::new(
            PluginManagerService::new(
                Arc::new(SqlitePluginManagerStore::new(storage.clone())),
                Arc::new(UnavailableRepositorySource),
                temp.path().join("plugins"),
                "1.0.0-alpha.1".parse().unwrap(),
                "1.0.0".parse().unwrap(),
            )
            .with_installation_ports(
                Arc::new(ApiRuntime { fail_runtime }),
                Arc::new(audiodown_server::plugin_manager_adapters::ConfiguredLifecycleRiskAuthorizer::new(
                    Default::default(),
                )),
            ),
        );
        let app = build_router(
            AppState::new(
                storage.clone(),
                "1.0.0-alpha.1".parse().unwrap(),
                Arc::new(UnavailableSupervisorClient),
            )
            .with_plugin_manager(manager),
        );
        Self {
            _temp: temp,
            app,
            storage,
            plugin_id,
        }
    }

    async fn request(&self, request: Request<Body>) -> axum::response::Response {
        self.app.clone().oneshot(request).await.unwrap()
    }
}

fn plugin_record(plugin_id: PluginId) -> PluginRecord {
    let now = Utc::now();
    PluginRecord {
        plugin_id,
        plugin_type: PluginType::Content,
        platform_id: "virtual".to_string(),
        name: "Virtual Content".to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_kind: "github".to_string(),
        source_ref: "https://github.com/example-owner/example-repository".to_string(),
        commit_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        repository_id: Some("example.plugins".to_string()),
        manifest_json: serde_json::json!({}),
        manifest_hash: "a".repeat(64),
        source_hash: Some("b".repeat(64)),
        image_id: Some(format!("sha256:{}", "c".repeat(64))),
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

struct ApiRuntime {
    fail_runtime: bool,
}

#[async_trait]
impl PluginRuntimeControl for ApiRuntime {
    async fn start(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        if self.fail_runtime {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        Ok(runtime_state(plugin_id, PluginStatus::Healthy))
    }

    async fn stop(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        Ok(runtime_state(plugin_id, PluginStatus::Stopped))
    }

    async fn inspect(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        Ok(runtime_state(plugin_id, PluginStatus::Healthy))
    }

    async fn remove(&self, plugin_id: &PluginId) -> Result<PluginRemoveResult, PluginManagerError> {
        Ok(PluginRemoveResult {
            plugin_id: plugin_id.clone(),
            removed_container: true,
            removed_image: true,
            removed_install_directory: true,
        })
    }

    async fn begin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn install_status(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn finalize_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn abort_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn list_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, PluginManagerError> {
        Ok(PluginInstallOperationList::new(Vec::new()))
    }

    async fn acknowledge_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }
}

fn runtime_state(plugin_id: &PluginId, status: PluginStatus) -> PluginRuntimeState {
    PluginRuntimeState {
        plugin_id: plugin_id.clone(),
        status,
        container_id: Some("container-virtual".to_string()),
        logs: Vec::new(),
    }
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source.find(start).unwrap();
    let tail = &source[start..];
    let end = tail.find(end).unwrap();
    &tail[..end]
}
