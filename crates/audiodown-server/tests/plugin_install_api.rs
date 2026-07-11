use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus};
use audiodown_plugin_manager::{
    github::GitHubRepositoryRef,
    service::{PluginManagerService, PluginRuntimeControl},
    DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use audiodown_server::{
    app::build_router,
    plugin_manager_adapters::{ConfiguredLifecycleRiskAuthorizer, SqlitePluginManagerStore},
    state::{AppState, UnavailableSupervisorClient},
};
use audiodown_storage::Storage;
use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact, PluginInstallOperation,
    PluginInstallOperationList, PluginInstallOperationState, PluginRemoveResult,
    PluginRuntimeState,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use flate2::{write::GzEncoder, Compression};
use secrecy::SecretString;
use serde_json::json;
use tar::Builder;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn exposes_development_mode_without_exposing_the_token() {
    let app = test_app(true, Some(SecretString::from("hidden-token".to_string()))).await;
    let response = app
        .oneshot(Request::get("/api/v1/system").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["developmentMode"], true);
    assert!(!String::from_utf8_lossy(&body).contains("hidden-token"));
}

#[tokio::test]
async fn maps_install_validation_errors_to_stable_codes() {
    let app = test_app(false, None).await;
    let snapshot_id = Uuid::new_v4();
    let response = app
        .oneshot(
            Request::post(format!(
                "/api/v1/plugin-repositories/{snapshot_id}/plugins/com.audiodown.virtual.content/install"
            ))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"allowLifecycleScripts":false}"#))
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["code"], "SNAPSHOT_NOT_FOUND");
}

#[tokio::test]
async fn rejects_unknown_install_fields() {
    let app = test_app(false, None).await;
    let response = app
        .oneshot(
            Request::post(format!(
                "/api/v1/plugin-repositories/{}/plugins/com.audiodown.virtual.content/install",
                Uuid::new_v4()
            ))
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"allowLifecycleScripts":false,"dockerfile":"forbidden"}"#,
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn enforces_fresh_risk_approval_and_developer_token_at_the_api_boundary() {
    let no_approval = RiskApiFixture::new(true, "valid-token").await;
    let response = no_approval.install(false, Some("valid-token")).await;
    assert_error(response, StatusCode::CONFLICT, "RISK_GRANT_REQUIRED").await;

    let mode_disabled = RiskApiFixture::new(false, "valid-token").await;
    let response = mode_disabled.install(true, Some("valid-token")).await;
    assert_error(response, StatusCode::FORBIDDEN, "DEVELOPER_MODE_REQUIRED").await;

    let bad_token = RiskApiFixture::new(true, "valid-token").await;
    let response = bad_token.install(true, Some("wrong-token")).await;
    assert_error(response, StatusCode::UNAUTHORIZED, "DEV_TOKEN_REQUIRED").await;

    let allowed = RiskApiFixture::new(true, "valid-token").await;
    let response = allowed.install(true, Some("valid-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["pluginId"], "com.audiodown.virtual.content");
    assert_eq!(body["status"], "installed");
}

async fn test_app(dev_mode: bool, token: Option<SecretString>) -> axum::Router {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    storage.migrate().await.unwrap();
    let state = AppState::new(
        storage,
        "1.0.0-alpha.1".parse().unwrap(),
        Arc::new(UnavailableSupervisorClient),
    )
    .with_development(dev_mode, token);
    build_router(state)
}

struct RiskApiFixture {
    _temp: TempDir,
    app: axum::Router,
    snapshot_id: Uuid,
}

impl RiskApiFixture {
    async fn new(dev_mode: bool, token: &str) -> Self {
        let temp = TempDir::new().unwrap();
        let plugin_data = temp.path().join("plugins");
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        storage.migrate().await.unwrap();
        let development = audiodown_server::state::DevelopmentConfig {
            enabled: dev_mode,
            token: Some(SecretString::new(token.to_string())),
        };
        let manager = Arc::new(
            PluginManagerService::new(
                Arc::new(SqlitePluginManagerStore::new(storage.clone())),
                Arc::new(RiskRepositorySource),
                plugin_data.clone(),
                "1.0.0-alpha.1".parse().unwrap(),
                "1.0.0".parse().unwrap(),
            )
            .with_installation_ports(
                Arc::new(PreparedArtifactRuntime { plugin_data }),
                Arc::new(ConfiguredLifecycleRiskAuthorizer::new(development.clone())),
            ),
        );
        let app = build_router(
            AppState::new(
                storage,
                "1.0.0-alpha.1".parse().unwrap(),
                Arc::new(UnavailableSupervisorClient),
            )
            .with_plugin_manager(manager)
            .with_development(development.enabled, development.token),
        );
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/plugin-repositories/inspect")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "url": "https://github.com/example-owner/example-repository"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let snapshot_id = Uuid::parse_str(body["snapshotId"].as_str().unwrap()).unwrap();
        Self {
            _temp: temp,
            app,
            snapshot_id,
        }
    }

    async fn install(&self, approved: bool, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::post(format!(
            "/api/v1/plugin-repositories/{}/plugins/com.audiodown.virtual.content/install",
            self.snapshot_id
        ))
        .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("x-audiodown-dev-token", token);
        }
        self.app
            .clone()
            .oneshot(
                request
                    .body(Body::from(
                        json!({"allowLifecycleScripts": approved}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}

struct RiskRepositorySource;

#[async_trait]
impl RepositorySource for RiskRepositorySource {
    async fn resolve_and_download(
        &self,
        _source: &GitHubRepositoryRef,
        destination: &Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError> {
        std::fs::create_dir_all(destination).map_err(|_| PluginManagerError::SnapshotIo)?;
        let archive_path = destination.join("snapshot.tar.gz");
        std::fs::write(&archive_path, risk_repository_archive())
            .map_err(|_| PluginManagerError::SnapshotIo)?;
        Ok(DownloadedSnapshot {
            commit_sha: COMMIT_SHA.to_string(),
            archive_path,
        })
    }
}

struct PreparedArtifactRuntime {
    plugin_data: PathBuf,
}

#[async_trait]
impl PluginRuntimeControl for PreparedArtifactRuntime {
    async fn start(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        Ok(PluginRuntimeState {
            plugin_id: plugin_id.clone(),
            status: PluginStatus::Healthy,
            container_id: None,
            logs: Vec::new(),
        })
    }

    async fn stop(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        Ok(PluginRuntimeState {
            plugin_id: plugin_id.clone(),
            status: PluginStatus::Stopped,
            container_id: None,
            logs: Vec::new(),
        })
    }

    async fn inspect(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        self.start(plugin_id).await
    }

    async fn remove(&self, plugin_id: &PluginId) -> Result<PluginRemoveResult, PluginManagerError> {
        Ok(PluginRemoveResult {
            plugin_id: plugin_id.clone(),
            removed_container: false,
            removed_image: true,
            removed_install_directory: true,
        })
    }

    async fn begin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Ok(install_operation(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Accepted,
            None,
        ))
    }

    async fn install_status(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        let artifact = self.artifact(operation_id)?;
        Ok(install_operation(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Built,
            Some(artifact),
        ))
    }

    async fn finalize_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        let artifact = self.artifact(operation_id)?;
        Ok(install_operation(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Finalized,
            Some(artifact),
        ))
    }

    async fn abort_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Ok(install_operation(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Aborted,
            None,
        ))
    }

    async fn list_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, PluginManagerError> {
        Ok(PluginInstallOperationList::new(Vec::new()))
    }

    async fn acknowledge_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        let mut operation = self.finalize_install(plugin_id, operation_id).await?;
        operation.acknowledged = true;
        Ok(operation)
    }
}

impl PreparedArtifactRuntime {
    fn artifact(&self, operation_id: Uuid) -> Result<PluginInstallArtifact, PluginManagerError> {
        let path = self
            .plugin_data
            .join("prepared")
            .join(format!("{operation_id}.json"));
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(path).map_err(|_| PluginManagerError::SnapshotIo)?,
        )
        .map_err(|_| PluginManagerError::InvalidStagingMetadata)?;
        Ok(PluginInstallArtifact {
            image_id: "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                .to_string(),
            repository_id: value["repositoryId"].as_str().unwrap().to_string(),
            commit_sha: value["commitSha"].as_str().unwrap().to_string(),
            source_hash: value["sourceHash"].as_str().unwrap().to_string(),
            manifest_hash: value["manifestHash"].as_str().unwrap().to_string(),
        })
    }
}

fn install_operation(
    plugin_id: &PluginId,
    operation_id: Uuid,
    state: PluginInstallOperationState,
    artifact: Option<PluginInstallArtifact>,
) -> PluginInstallOperation {
    PluginInstallOperation {
        operation_id,
        plugin_id: plugin_id.clone(),
        state,
        artifact,
        build_logs: vec![PluginBuildLog {
            sequence: 1,
            stream: PluginBuildLogStream::Stdout,
            message: "virtual build complete".to_string(),
        }],
        error_code: None,
        acknowledged: false,
    }
}

fn risk_repository_archive() -> Vec<u8> {
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
            "network": {"allowedHosts": []},
            "build": {
                "npmLifecycleScripts": {
                    "required": true,
                    "reason": "Generate a deterministic local file"
                }
            }
        })
        .to_string(),
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/package.json",
        r#"{"name":"virtual-content","version":"1.0.0","scripts":{"install":"node scripts/install.js"}}"#,
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/package-lock.json",
        r#"{"name":"virtual-content","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"virtual-content","version":"1.0.0","hasInstallScript":true}}}"#,
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/src/index.js",
        "export default {};\n",
    );
    append(
        &mut archive,
        "root/plugins/virtual-content/scripts/install.js",
        "process.stdout.write('ok');\n",
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

async fn assert_error(response: axum::response::Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(response_json(response).await["code"], code);
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}
