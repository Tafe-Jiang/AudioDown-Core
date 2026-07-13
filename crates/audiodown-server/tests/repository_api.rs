use std::{io::Write, path::Path, sync::Arc};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_manager::{
    service::{PluginManagerService, PluginStateStore},
    DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use audiodown_server::{
    app::build_router,
    state::{AppState, UnavailableSupervisorClient},
};
use audiodown_storage::Storage;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use flate2::{write::GzEncoder, Compression};
use serde_json::json;
use tar::Builder;
use tempfile::TempDir;
use tower::ServiceExt;

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn exposes_repository_inspection_without_supervisor_access() {
    let fixture = ApiFixture::new(Arc::new(FixtureSource::valid())).await;
    let response = fixture
        .post(json!({
            "url": "https://github.com/example-owner/example-repository"
        }))
        .await;
    assert_eq!(response.0, StatusCode::OK);
    assert!(uuid::Uuid::parse_str(response.1["snapshotId"].as_str().unwrap()).is_ok());
    assert_eq!(
        response.1["repository"],
        json!({
            "id": "example.plugins",
            "name": "Example Plugins",
            "sourceUrl": "https://github.com/example-owner/example-repository",
            "commitSha": COMMIT_SHA
        })
    );
    assert_eq!(
        response.1["plugins"],
        json!([{
            "pluginId": "com.audiodown.virtual.content",
            "name": "Virtual Content",
            "version": "1.0.0",
            "pluginType": "content",
            "alreadyInstalled": false,
            "requiresLifecycleScriptGrant": false,
            "lifecycleScriptReason": null,
            "credentials": {
                "providedScopes": [],
                "requiredScopes": [],
                "optionalScopes": []
            }
        }])
    );
}

#[tokio::test]
async fn returns_stable_repository_inspection_errors() {
    let valid = ApiFixture::new(Arc::new(FixtureSource::valid())).await;
    let invalid_url = valid.post(json!({"url": "not-a-url"})).await;
    assert_eq!(invalid_url.0, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_url.1["code"], "INVALID_REPOSITORY_URL");

    let unavailable = ApiFixture::new(Arc::new(FixtureSource::failing())).await;
    let unavailable = unavailable
        .post(json!({
            "url": "https://github.com/example-owner/example-repository"
        }))
        .await;
    assert_eq!(unavailable.0, StatusCode::BAD_GATEWAY);
    assert_eq!(unavailable.1["code"], "REPOSITORY_UNAVAILABLE");

    let invalid = ApiFixture::new(Arc::new(FixtureSource::invalid())).await;
    let invalid = invalid
        .post(json!({
            "url": "https://github.com/example-owner/example-repository"
        }))
        .await;
    assert_eq!(invalid.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invalid.1["code"], "INVALID_REPOSITORY");
}

#[tokio::test]
async fn rejects_unknown_fields_and_urls_over_five_hundred_twelve_bytes() {
    let fixture = ApiFixture::new(Arc::new(FixtureSource::valid())).await;
    let unknown = fixture
        .post(json!({
            "url": "https://github.com/example-owner/example-repository",
            "token": "must-not-be-accepted"
        }))
        .await;
    assert_eq!(unknown.0, StatusCode::UNPROCESSABLE_ENTITY);

    let long = fixture.post(json!({"url": "x".repeat(513)})).await;
    assert_eq!(long.0, StatusCode::BAD_REQUEST);
    assert_eq!(long.1["code"], "INVALID_REPOSITORY_URL");
}

struct ApiFixture {
    _temp: TempDir,
    app: axum::Router,
}

impl ApiFixture {
    async fn new(source: Arc<dyn RepositorySource>) -> Self {
        let temp = TempDir::new().unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        storage.migrate().await.unwrap();
        let manager = Arc::new(PluginManagerService::new(
            Arc::new(FakeStateStore),
            source,
            temp.path().join("plugins"),
            "1.0.0-alpha.1".parse().unwrap(),
            "1.0.0".parse().unwrap(),
        ));
        let state = AppState::new(
            storage,
            "1.0.0-alpha.1".parse().unwrap(),
            Arc::new(UnavailableSupervisorClient),
        )
        .with_plugin_manager(manager);
        Self {
            _temp: temp,
            app: build_router(state),
        }
    }

    async fn post(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/api/v1/plugin-repositories/inspect")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value = serde_json::from_slice(&body)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&body)}));
        (status, value)
    }
}

struct FakeStateStore;

#[async_trait]
impl PluginStateStore for FakeStateStore {
    async fn is_installed(&self, _plugin_id: &PluginId) -> Result<bool, PluginManagerError> {
        Ok(false)
    }
}

struct FixtureSource {
    archive: Vec<u8>,
    failure: bool,
}

impl FixtureSource {
    fn valid() -> Self {
        Self {
            archive: valid_repository_archive(),
            failure: false,
        }
    }

    fn invalid() -> Self {
        Self {
            archive: b"invalid archive".to_vec(),
            failure: false,
        }
    }

    fn failing() -> Self {
        Self {
            archive: Vec::new(),
            failure: true,
        }
    }
}

#[async_trait]
impl RepositorySource for FixtureSource {
    async fn resolve_and_download(
        &self,
        _source: &audiodown_plugin_manager::github::GitHubRepositoryRef,
        destination: &Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError> {
        if self.failure {
            return Err(PluginManagerError::RepositoryRequest);
        }
        std::fs::create_dir_all(destination).map_err(|_| PluginManagerError::SnapshotIo)?;
        let archive_path = destination.join("snapshot.tar.gz");
        std::fs::write(&archive_path, &self.archive).map_err(|_| PluginManagerError::SnapshotIo)?;
        Ok(DownloadedSnapshot {
            commit_sha: COMMIT_SHA.to_string(),
            archive_path,
        })
    }
}

fn valid_repository_archive() -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = Builder::new(encoder);
    append(
        &mut archive,
        "root/audiodown-repository.json",
        &json!({
            "schemaVersion": "1.0",
            "repository": {"id": "example.plugins", "name": "Example Plugins"},
            "plugins": [{"path": "plugins/virtual-content"}]
        })
        .to_string(),
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/audiodown-plugin.json",
        &json!({
            "schemaVersion": "1.0",
            "id": "com.audiodown.virtual.content",
            "name": "Virtual Content",
            "version": "1.0.0",
            "type": "content",
            "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
            "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
            "platform": {"id": "virtual", "name": "Virtual"},
            "capabilities": ["content.search"],
            "network": {"allowedHosts": []}
        })
        .to_string(),
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/package.json",
        r#"{"name":"virtual-content","version":"1.0.0"}"#,
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/package-lock.json",
        r#"{"name":"virtual-content","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"virtual-content","version":"1.0.0"}}}"#,
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/src/index.js",
        "export default {};\n",
    );
    archive.into_inner().unwrap().finish().unwrap()
}

fn append<W: Write>(archive: &mut Builder<W>, path: &str, content: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive
        .append_data(&mut header, path, content.as_bytes())
        .unwrap();
}
