use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::{
    content::{
        AlbumDetail, AlbumGetRequest, AlbumGetResult, CategoriesResult, CategoryItem, ContentItem,
        ContentMethod, ContentResourceType, DiscoverLayout, DiscoverRequest, DiscoverResult,
        DiscoverSection, SearchRequest, SearchResult, TrackItem, TracksListRequest,
        TracksListResult,
    },
    error::{PluginErrorCode, PluginErrorData},
    manifest::PluginType,
    rpc::{JsonRpcError, JsonRpcResponse},
};
use audiodown_plugin_manager::service::PluginManagerService;
use audiodown_server::{
    app::build_router,
    plugin_manager_adapters::{
        ConfiguredLifecycleRiskAuthorizer, SqlitePluginManagerStore, SupervisorPluginRuntime,
        UnavailableRepositorySource,
    },
    state::AppState,
    supervisor::{
        PluginInstallOperation, PluginInstallOperationList, PluginRemoveResult, PluginRpcResult,
        PluginRuntimeState, SupervisorClient, SupervisorError, SupervisorHealth,
    },
};
use audiodown_storage::{PluginRecord, Storage};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use chrono::Utc;
use semver::Version;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const VIRTUAL_PRIMARY: &str = "com.audiodown.virtual.primary";
const VIRTUAL_BACKUP: &str = "com.audiodown.virtual.backup";
const CATALOG_PRIMARY: &str = "com.audiodown.catalog.primary";

#[tokio::test]
async fn returns_typed_empty_states_and_stable_validation_errors() {
    let fixture = TestApp::new(false).await;

    for uri in [
        "/api/v1/search?q=needle",
        "/api/v1/discover",
        "/api/v1/categories",
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["emptyState"]["reason"], "NO_CONTENT_PLUGINS");
        assert_eq!(body["failures"], json!([]));
    }

    for uri in [
        "/api/v1/search",
        "/api/v1/search?q=%20",
        "/api/v1/search?q=needle&limit=0",
        "/api/v1/search?q=needle&pluginId=INVALID",
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["code"].as_str().unwrap().starts_with("INVALID_"));
        assert!(!body.to_string().contains("raw-plugin-secret"));
    }
}

#[tokio::test]
async fn aggregates_filters_fallback_partial_failures_and_cursor_continuation() {
    let fixture = TestApp::new(true).await;

    let (status, filtered) = fixture
        .get("/api/v1/search?q=fallback&platformId=virtual")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["items"][0]["source"]["pluginId"], VIRTUAL_BACKUP);
    assert_eq!(filtered["failures"][0]["code"], "RATE_LIMITED");
    let cursor = filtered["nextCursor"].as_str().unwrap().to_string();

    let (status, continued) = fixture
        .get(&format!(
            "/api/v1/search?q=fallback&platformId=virtual&cursor={cursor}"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(continued["items"][0]["source"]["pluginId"], VIRTUAL_BACKUP);
    assert_eq!(continued["items"][0]["item"]["resourceId"], "backup-page-2");
    assert!(continued["nextCursor"].is_null());

    let (status, partial) = fixture.get("/api/v1/search?q=partial").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(partial["items"].as_array().unwrap().len(), 1);
    assert_eq!(partial["items"][0]["source"]["platformId"], "virtual");
    assert_eq!(partial["failures"].as_array().unwrap().len(), 1);
    assert_eq!(partial["failures"][0]["code"], "RESOURCE_ACCESS_DENIED");
    assert!(!partial.to_string().contains("raw-plugin-secret"));
}

#[tokio::test]
async fn exposes_discover_categories_source_bound_album_tracks_and_settings() {
    let fixture = TestApp::new(true).await;

    let (status, discover) = fixture
        .get("/api/v1/discover?pluginId=com.audiodown.virtual.primary")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(discover["sections"].as_array().unwrap().len(), 1);
    assert_eq!(
        discover["sections"][0]["source"]["pluginId"],
        VIRTUAL_PRIMARY
    );

    let (status, categories) = fixture.get("/api/v1/categories").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(categories["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        categories["items"][0]["source"]["pluginId"],
        CATALOG_PRIMARY
    );

    let (status, album) = fixture
        .json_request(
            "POST",
            "/api/v1/albums/get",
            json!({"pluginId": VIRTUAL_BACKUP, "resourceId": "album-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(album["source"]["pluginId"], VIRTUAL_BACKUP);
    assert_eq!(album["album"]["resourceId"], "album-1");

    let (status, missing_album) = fixture
        .json_request(
            "POST",
            "/api/v1/albums/get",
            json!({"pluginId": VIRTUAL_BACKUP, "resourceId": "missing"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing_album["code"], "RESOURCE_NOT_FOUND");
    assert!(!missing_album.to_string().contains("raw-plugin-secret"));

    let (status, tracks) = fixture
        .json_request(
            "POST",
            "/api/v1/tracks/list",
            json!({
                "pluginId": VIRTUAL_BACKUP,
                "albumResourceId": "album-1",
                "limit": 20
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tracks["source"]["pluginId"], VIRTUAL_BACKUP);
    assert_eq!(tracks["items"][0]["resourceId"], "track-page-1");
    assert_eq!(tracks["nextCursor"], "track-next");

    let (status, tracks_page_2) = fixture
        .json_request(
            "POST",
            "/api/v1/tracks/list",
            json!({
                "pluginId": VIRTUAL_BACKUP,
                "albumResourceId": "album-1",
                "cursor": "track-next",
                "limit": 20
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tracks_page_2["items"][0]["resourceId"], "track-page-2");
    assert!(tracks_page_2["nextCursor"].is_null());

    let (status, settings) = fixture
        .json_request(
            "PATCH",
            "/api/v1/plugins/com.audiodown.virtual.backup/content-settings",
            json!({"searchEnabled": false, "discoverEnabled": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["searchEnabled"], false);
    assert_eq!(settings["discoverEnabled"], true);

    let (status, default) = fixture
        .json_request(
            "PUT",
            "/api/v1/platforms/virtual/default-content-plugin",
            json!({"pluginId": VIRTUAL_BACKUP}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(default["platformId"], "virtual");
    assert_eq!(default["pluginId"], VIRTUAL_BACKUP);

    let (status, error) = fixture
        .json_request(
            "PUT",
            "/api/v1/platforms/catalog/default-content-plugin",
            json!({"pluginId": VIRTUAL_BACKUP}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "INVALID_CONTENT_DEFAULT");
}

#[tokio::test]
async fn records_redacted_structured_content_call_logs() {
    let fixture = TestApp::new(true).await;
    let (status, _) = fixture.get("/api/v1/search?q=partial").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = fixture
        .json_request(
            "POST",
            "/api/v1/albums/get",
            json!({"pluginId": VIRTUAL_BACKUP, "resourceId": "missing"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, logs) = fixture.get("/api/v1/logs?limit=100").await;
    assert_eq!(status, StatusCode::OK);
    let items = logs["items"].as_array().unwrap();
    let completion = items
        .iter()
        .find(|item| {
            item["component"] == "content-api" && item["errorCode"] == "RESOURCE_ACCESS_DENIED"
        })
        .expect("content completion log");
    assert!(completion["requestId"].as_str().is_some());
    assert!(completion["pluginId"].as_str().is_some());
    assert!(completion["platformId"].as_str().is_some());
    assert!(completion["context"]["method"].as_str().is_some());
    assert!(completion["context"]["durationMs"].as_u64().is_some());
    assert!(completion["context"]["resultCount"].as_u64().is_some());
    assert_eq!(completion["errorCode"], "RESOURCE_ACCESS_DENIED");
    assert!(items.iter().any(|item| {
        item["component"] == "content-api"
            && item["context"]["method"] == "content.album.get"
            && item["errorCode"] == "RESOURCE_NOT_FOUND"
    }));
    let serialized = logs.to_string();
    assert!(!serialized.contains("raw-plugin-secret"));
    assert!(!serialized.contains("untrusted-plugin-stack"));
}

struct TestApp {
    router: axum::Router,
}

impl TestApp {
    async fn new(with_plugins: bool) -> Self {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        storage.migrate().await.unwrap();
        if with_plugins {
            for record in [
                plugin_record(VIRTUAL_PRIMARY, "virtual", 100),
                plugin_record(VIRTUAL_BACKUP, "virtual", 10),
                plugin_record(CATALOG_PRIMARY, "catalog", 10),
            ] {
                storage.plugins().upsert(&record).await.unwrap();
            }
            storage
                .content_routing()
                .set_default("virtual", &PluginId::parse(VIRTUAL_PRIMARY).unwrap())
                .await
                .unwrap();
        }
        let supervisor = Arc::new(FixtureSupervisor::default());
        let manager = Arc::new(
            PluginManagerService::new(
                Arc::new(SqlitePluginManagerStore::new(storage.clone())),
                Arc::new(UnavailableRepositorySource),
                std::env::temp_dir().join(format!("audiodown-content-api-{}", Uuid::new_v4())),
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
        Self {
            router: build_router(state),
        }
    }

    async fn get(&self, uri: &str) -> (StatusCode, serde_json::Value) {
        response_json(
            self.router
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await
    }

    async fn json_request(
        &self,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        response_json(self.router.clone().oneshot(request).await.unwrap()).await
    }
}

async fn response_json(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[derive(Default)]
struct FixtureSupervisor {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl SupervisorClient for FixtureSupervisor {
    async fn ping(&self) -> Result<SupervisorHealth, SupervisorError> {
        Ok(SupervisorHealth {
            service: "fixture-supervisor".to_string(),
        })
    }

    async fn start_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        Ok(runtime_state(plugin_id, PluginStatus::Healthy))
    }

    async fn stop_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        Ok(runtime_state(plugin_id, PluginStatus::Stopped))
    }

    async fn inspect_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        Ok(runtime_state(plugin_id, PluginStatus::Healthy))
    }

    async fn invoke_plugin(
        &self,
        plugin_id: &PluginId,
        method: ContentMethod,
        params: serde_json::Value,
    ) -> Result<PluginRpcResult, SupervisorError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{plugin_id}:{}", method.capability()));
        let response = match method {
            ContentMethod::Search => search_response(plugin_id, params),
            ContentMethod::Discover => discover_response(plugin_id, params),
            ContentMethod::Categories => categories_response(plugin_id),
            ContentMethod::AlbumGet => album_response(plugin_id, params),
            ContentMethod::TracksList => tracks_response(plugin_id, params),
        };
        PluginRpcResult::new(response).map_err(|_| SupervisorError::InvalidResponse)
    }

    async fn begin_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn plugin_install_status(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn finalize_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn abort_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn list_plugin_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn acknowledge_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn remove_plugin(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRemoveResult, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }
}

fn search_response(plugin_id: &PluginId, params: serde_json::Value) -> JsonRpcResponse {
    let request: SearchRequest = serde_json::from_value(params).unwrap();
    if plugin_id.as_str() == VIRTUAL_PRIMARY && request.query == "fallback" {
        return plugin_error(PluginErrorCode::RateLimited);
    }
    if plugin_id.as_str() == CATALOG_PRIMARY && request.query == "partial" {
        return plugin_error(PluginErrorCode::ResourceAccessDenied);
    }
    let (resource_id, next_cursor) = if request.cursor.is_some() {
        (
            if plugin_id.as_str() == VIRTUAL_BACKUP {
                "backup-page-2"
            } else {
                "primary-page-2"
            },
            None,
        )
    } else {
        (
            if plugin_id.as_str() == VIRTUAL_BACKUP {
                "backup-page-1"
            } else if plugin_id.as_str() == CATALOG_PRIMARY {
                "catalog-page-1"
            } else {
                "primary-page-1"
            },
            Some(format!("{}-next", plugin_id.as_str())),
        )
    };
    plugin_success(SearchResult {
        items: vec![content_item(resource_id)],
        next_cursor,
    })
}

fn discover_response(_plugin_id: &PluginId, params: serde_json::Value) -> JsonRpcResponse {
    let _: DiscoverRequest = serde_json::from_value(params).unwrap();
    plugin_success(DiscoverResult {
        sections: vec![DiscoverSection {
            id: "featured".to_string(),
            title: "Featured".to_string(),
            layout: DiscoverLayout::AlbumGrid,
            items: vec![content_item("discover-album")],
        }],
        next_cursor: None,
    })
}

fn categories_response(_plugin_id: &PluginId) -> JsonRpcResponse {
    plugin_success(CategoriesResult {
        items: vec![CategoryItem {
            resource_id: "category-1".to_string(),
            canonical_id: Some("category-canonical".to_string()),
            title: "Category".to_string(),
            description: None,
        }],
    })
}

fn album_response(_plugin_id: &PluginId, params: serde_json::Value) -> JsonRpcResponse {
    let request: AlbumGetRequest = serde_json::from_value(params).unwrap();
    if request.resource_id == "missing" {
        return plugin_error(PluginErrorCode::ResourceNotFound);
    }
    plugin_success(AlbumGetResult {
        album: AlbumDetail {
            resource_id: request.resource_id,
            canonical_id: Some("album-canonical".to_string()),
            title: "Fixture album".to_string(),
            creator: Some("Fixture creator".to_string()),
            description: None,
            track_count: Some(2),
        },
    })
}

fn tracks_response(_plugin_id: &PluginId, params: serde_json::Value) -> JsonRpcResponse {
    let request: TracksListRequest = serde_json::from_value(params).unwrap();
    let second_page = request.cursor.is_some();
    plugin_success(TracksListResult {
        items: vec![TrackItem {
            resource_id: if second_page {
                "track-page-2".to_string()
            } else {
                "track-page-1".to_string()
            },
            canonical_id: None,
            title: "Fixture track".to_string(),
            subtitle: None,
            sequence: Some(if second_page { 2 } else { 1 }),
            duration_seconds: Some(60),
        }],
        next_cursor: (!second_page).then(|| "track-next".to_string()),
    })
}

fn plugin_success(result: impl serde::Serialize) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: "fixture-request".to_string(),
        result: Some(serde_json::to_value(result).unwrap()),
        error: None,
    }
}

fn plugin_error(code: PluginErrorCode) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: "fixture-request".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code: -32000,
            message: "untrusted-plugin-stack".to_string(),
            data: Some(
                serde_json::to_value(PluginErrorData {
                    code,
                    summary: "raw-plugin-secret".to_string(),
                    retry_after_seconds: None,
                })
                .unwrap(),
            ),
        }),
    }
}

fn content_item(resource_id: &str) -> ContentItem {
    ContentItem {
        resource_type: ContentResourceType::Album,
        resource_id: resource_id.to_string(),
        canonical_id: None,
        title: resource_id.to_string(),
        subtitle: None,
        description: None,
    }
}

fn runtime_state(plugin_id: &PluginId, status: PluginStatus) -> PluginRuntimeState {
    PluginRuntimeState {
        plugin_id: plugin_id.clone(),
        status,
        container_id: Some(format!("fixture-{plugin_id}")),
        logs: Vec::new(),
    }
}

fn plugin_record(plugin_id: &str, platform_id: &str, priority: i64) -> PluginRecord {
    let now = Utc::now();
    PluginRecord {
        plugin_id: PluginId::parse(plugin_id).unwrap(),
        plugin_type: PluginType::Content,
        platform_id: platform_id.to_string(),
        name: plugin_id.to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_kind: "fixture".to_string(),
        source_ref: "virtual-content-api-fixture".to_string(),
        commit_sha: None,
        repository_id: None,
        manifest_json: json!({
            "schemaVersion": "1.0",
            "id": plugin_id,
            "name": plugin_id,
            "version": "1.0.0",
            "type": "content",
            "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
            "compatibility": {"pluginApi": "^1.0.0", "core": "^1.0.0"},
            "platform": {"id": platform_id, "name": platform_id},
            "capabilities": [
                "content.search",
                "content.discover",
                "content.categories",
                "content.album.get",
                "content.tracks.list"
            ],
            "network": {"allowedHosts": []}
        }),
        manifest_hash: "a".repeat(64),
        source_hash: None,
        image_id: Some(format!("audiodown/fixture-{platform_id}:dev")),
        status: PluginStatus::Installed,
        run_mode: RunMode::OnDemand,
        priority,
        enabled: true,
        last_error: None,
        install_operation_id: None,
        last_used_at: None,
        installed_at: now,
        updated_at: now,
    }
}
