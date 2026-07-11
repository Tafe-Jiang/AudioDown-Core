use audiodown_domain::{
    log::{LogLevel, StructuredLog},
    plugin::{PluginId, PluginStatus, RunMode},
};
use audiodown_plugin_api::manifest::PluginManifest;
use audiodown_plugin_manager::service::{InstallError, InstallPluginCommand, LifecycleRiskInput};
use audiodown_storage::PluginRecord;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::Utc;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    plugin_manager_adapters::secret_matches,
    routes::{internal_error, ApiError, ApiResult},
    state::AppState,
    supervisor::{PluginRuntimeState, SupervisorError},
};

const VIRTUAL_PLUGIN_ID: &str = "com.audiodown.virtual.content";
const VIRTUAL_IMAGE_ID: &str = "audiodown/plugin-virtual:dev";

#[derive(Debug, Serialize)]
pub struct PluginListResponse {
    pub items: Vec<PluginItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginItem {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub status: PluginStatus,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyStateResponse {
    pub reason: &'static str,
    pub title: &'static str,
    pub action_label: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

pub async fn list(State(state): State<AppState>) -> ApiResult<PluginListResponse> {
    let records = state
        .storage
        .plugins()
        .list()
        .await
        .map_err(internal_error)?;
    let items = records
        .into_iter()
        .map(|record| PluginItem {
            plugin_id: record.plugin_id.to_string(),
            name: record.name,
            version: record.version,
            status: record.status,
            enabled: record.enabled,
        })
        .collect();
    Ok(Json(PluginListResponse { items }))
}

pub async fn discover() -> Json<EmptyStateResponse> {
    Json(empty_state())
}

pub async fn search(Query(query): Query<SearchQuery>) -> Json<EmptyStateResponse> {
    let _ = query.q;
    Json(empty_state())
}

pub async fn start(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let record = state
        .storage
        .plugins()
        .get(&plugin_id)
        .await
        .map_err(internal_error)?;
    state
        .storage
        .plugins()
        .set_status(&plugin_id, PluginStatus::Starting)
        .await
        .map_err(internal_error)?;

    match state.supervisor.start_plugin(&plugin_id).await {
        Ok(runtime) => {
            if let Some(record) = &record {
                persist_runtime_logs(&state, record, &runtime).await?;
            }
            state
                .storage
                .plugins()
                .set_status(&plugin_id, PluginStatus::Healthy)
                .await
                .map_err(internal_error)?;
            Ok(Json(runtime))
        }
        Err(error) => {
            let _ = state
                .storage
                .plugins()
                .set_status(&plugin_id, PluginStatus::Unhealthy)
                .await;
            Err(supervisor_error(error))
        }
    }
}

pub async fn stop(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    match state.supervisor.stop_plugin(&plugin_id).await {
        Ok(runtime) => {
            state
                .storage
                .plugins()
                .set_status(&plugin_id, PluginStatus::Stopped)
                .await
                .map_err(internal_error)?;
            Ok(Json(runtime))
        }
        Err(error) => {
            let _ = state
                .storage
                .plugins()
                .set_status(&plugin_id, PluginStatus::Unhealthy)
                .await;
            Err(supervisor_error(error))
        }
    }
}

pub async fn runtime(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    require_plugin(&state, &plugin_id).await?;
    let runtime = state
        .supervisor
        .inspect_plugin(&plugin_id)
        .await
        .map_err(supervisor_error)?;
    state
        .storage
        .plugins()
        .set_status(&plugin_id, runtime.status)
        .await
        .map_err(internal_error)?;
    Ok(Json(runtime))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallPluginRequest {
    #[serde(default)]
    pub allow_lifecycle_scripts: bool,
}

pub async fn install(
    State(state): State<AppState>,
    Path((snapshot_id, plugin_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Json(request): Json<InstallPluginRequest>,
) -> ApiResult<PluginItem> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let developer_token = headers
        .get("x-audiodown-dev-token")
        .and_then(|value| value.to_str().ok())
        .map(|value| SecretString::new(value.to_string()));
    let installed = state
        .plugin_manager
        .install(InstallPluginCommand {
            snapshot_id,
            plugin_id,
            lifecycle_risk: LifecycleRiskInput {
                explicitly_approved: request.allow_lifecycle_scripts,
                developer_token,
            },
        })
        .await
        .map_err(install_error)?;

    Ok(Json(PluginItem {
        plugin_id: installed.plugin_id.to_string(),
        name: installed.name,
        version: installed.version,
        status: installed.status,
        enabled: true,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterFixtureRequest {
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub image_id: String,
}

pub async fn register_fixture(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RegisterFixtureRequest>,
) -> ApiResult<PluginItem> {
    if !state.development.enabled {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "API route was not found",
        ));
    }
    let supplied_token = headers
        .get("x-audiodown-dev-token")
        .and_then(|value| value.to_str().ok());
    if !token_matches(state.development.token.as_ref(), supplied_token) {
        return Err(api_error(
            StatusCode::UNAUTHORIZED,
            "DEV_TOKEN_REQUIRED",
            "A valid development token is required",
        ));
    }
    if request.manifest.id.as_str() != VIRTUAL_PLUGIN_ID
        || request.image_id != VIRTUAL_IMAGE_ID
        || !is_lower_hex_sha256(&request.manifest_hash)
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_FIXTURE",
            "The development fixture is not allowed",
        ));
    }

    let now = Utc::now();
    let manifest_json = serde_json::to_value(&request.manifest).map_err(internal_error)?;
    let record = PluginRecord {
        plugin_id: request.manifest.id.clone(),
        plugin_type: request.manifest.plugin_type,
        platform_id: request.manifest.platform.id.clone(),
        name: request.manifest.name.clone(),
        version: request.manifest.version.to_string(),
        protocol_version: request.manifest.schema_version.clone(),
        source_kind: "fixture".to_string(),
        source_ref: "virtual-contract-fixture".to_string(),
        commit_sha: None,
        repository_id: None,
        manifest_json,
        manifest_hash: request.manifest_hash,
        source_hash: None,
        image_id: Some(request.image_id),
        status: PluginStatus::Installed,
        run_mode: RunMode::OnDemand,
        priority: 100,
        enabled: true,
        last_error: None,
        install_operation_id: None,
        last_used_at: None,
        installed_at: now,
        updated_at: now,
    };
    state
        .storage
        .plugins()
        .upsert(&record)
        .await
        .map_err(internal_error)?;

    Ok(Json(PluginItem {
        plugin_id: record.plugin_id.to_string(),
        name: record.name,
        version: record.version,
        status: record.status,
        enabled: record.enabled,
    }))
}

fn token_matches(expected: Option<&SecretString>, supplied: Option<&str>) -> bool {
    let (Some(expected), Some(supplied)) = (expected, supplied) else {
        return false;
    };
    secret_matches(expected, supplied)
}

fn install_error(error: InstallError) -> (StatusCode, Json<ApiError>) {
    match error {
        InstallError::SnapshotNotFound => api_error(
            StatusCode::NOT_FOUND,
            "SNAPSHOT_NOT_FOUND",
            "Staged repository snapshot was not found",
        ),
        InstallError::PluginNotInSnapshot => api_error(
            StatusCode::NOT_FOUND,
            "PLUGIN_NOT_IN_SNAPSHOT",
            "Plugin is not present in the staged repository",
        ),
        InstallError::PluginAlreadyInstalled => api_error(
            StatusCode::CONFLICT,
            "PLUGIN_ALREADY_INSTALLED",
            "Plugin is already installed",
        ),
        InstallError::PluginOperationInProgress => api_error(
            StatusCode::CONFLICT,
            "PLUGIN_OPERATION_IN_PROGRESS",
            "Another operation is already running for this plugin",
        ),
        InstallError::RiskGrantRequired => api_error(
            StatusCode::CONFLICT,
            "RISK_GRANT_REQUIRED",
            "Lifecycle-script approval is required",
        ),
        InstallError::DeveloperModeRequired => api_error(
            StatusCode::FORBIDDEN,
            "DEVELOPER_MODE_REQUIRED",
            "Developer mode is required",
        ),
        InstallError::DevTokenRequired => api_error(
            StatusCode::UNAUTHORIZED,
            "DEV_TOKEN_REQUIRED",
            "A valid development token is required",
        ),
        InstallError::BuildFailed => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "PLUGIN_BUILD_FAILED",
            "Plugin build failed",
        ),
        InstallError::InstallTimeout => api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "INSTALL_TIMEOUT",
            "Plugin installation timed out",
        ),
        InstallError::RuntimeUnavailable => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SUPERVISOR_UNAVAILABLE",
            "Plugin management service is unavailable",
        ),
        InstallError::ArtifactMismatch | InstallError::Internal => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PLUGIN_INSTALL_FAILED",
            "Plugin installation failed",
        ),
    }
}

fn empty_state() -> EmptyStateResponse {
    EmptyStateResponse {
        reason: "NO_CONTENT_PLUGINS",
        title: "尚未安装内容插件",
        action_label: "添加 GitHub 插件仓库",
    }
}

fn parse_plugin_id(plugin_id: String) -> Result<PluginId, (StatusCode, Json<ApiError>)> {
    PluginId::parse(plugin_id).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                code: "INVALID_PLUGIN_ID",
                message: error.to_string(),
            }),
        )
    })
}

async fn require_plugin(
    state: &AppState,
    plugin_id: &PluginId,
) -> Result<PluginRecord, (StatusCode, Json<ApiError>)> {
    state
        .storage
        .plugins()
        .get(plugin_id)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| {
            api_error(
                StatusCode::NOT_FOUND,
                "PLUGIN_NOT_FOUND",
                "Plugin is not installed",
            )
        })
}

async fn persist_runtime_logs(
    state: &AppState,
    record: &PluginRecord,
    runtime: &PluginRuntimeState,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    for entry in &runtime.logs {
        let log = StructuredLog {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            level: parse_log_level(&entry.level),
            component: "plugin-runtime".to_string(),
            message: audiodown_logging::redact_text(&entry.message),
            plugin_id: Some(record.plugin_id.to_string()),
            plugin_version: Some(record.version.clone()),
            platform_id: Some(record.platform_id.clone()),
            request_id: None,
            task_id: None,
            container_id: runtime.container_id.clone(),
            error_code: None,
            context: audiodown_logging::redact_json(&entry.context),
        };
        state
            .storage
            .logs()
            .append(&log)
            .await
            .map_err(internal_error)?;
    }
    Ok(())
}

fn parse_log_level(level: &str) -> LogLevel {
    match level {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn api_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            code,
            message: message.to_string(),
        }),
    )
}

fn supervisor_error(error: SupervisorError) -> (StatusCode, Json<ApiError>) {
    if error.is_unavailable() {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                code: "SUPERVISOR_UNAVAILABLE",
                message: "Plugin management service is unavailable".to_string(),
            }),
        )
    } else {
        tracing::warn!(error = %error, "Supervisor rejected plugin lifecycle request");
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                code: "SUPERVISOR_ERROR",
                message: "Plugin lifecycle request failed".to_string(),
            }),
        )
    }
}
