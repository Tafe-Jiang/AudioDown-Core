use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginManifest;
use audiodown_plugin_manager::service::{
    InstallError, InstallPluginCommand, InstallPluginRecord, LifecycleRiskInput,
    PluginManagementError, UpdatePluginSettingsCommand,
};
use audiodown_storage::PluginRecord;
use axum::{
    extract::{Path, State},
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
    supervisor::PluginRuntimeState,
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
    pub plugin_type: audiodown_plugin_api::manifest::PluginType,
    pub platform_id: String,
    pub name: String,
    pub version: String,
    pub status: PluginStatus,
    pub enabled: bool,
    pub run_mode: RunMode,
    pub priority: i64,
    pub source_url: String,
    pub commit_sha: String,
    pub capabilities: Vec<String>,
    pub search_enabled: Option<bool>,
    pub discover_enabled: Option<bool>,
    pub is_default_content_plugin: bool,
}

pub async fn list(State(state): State<AppState>) -> ApiResult<PluginListResponse> {
    let records = state
        .storage
        .plugins()
        .list()
        .await
        .map_err(internal_error)?;
    let mut items = Vec::with_capacity(records.len());
    for record in records {
        items.push(plugin_item_from_storage(&state, record).await?);
    }
    Ok(Json(PluginListResponse { items }))
}

pub async fn start(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let record = state
        .plugin_manager
        .start(&plugin_id)
        .await
        .map_err(management_error)?;
    Ok(Json(runtime_state(&record)))
}

pub async fn stop(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let record = state
        .plugin_manager
        .stop(&plugin_id)
        .await
        .map_err(management_error)?;
    Ok(Json(runtime_state(&record)))
}

pub async fn runtime(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<PluginRuntimeState> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let record = state
        .plugin_manager
        .inspect_runtime(&plugin_id)
        .await
        .map_err(management_error)?;
    Ok(Json(runtime_state(&record)))
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

    Ok(Json(plugin_item_from_manager(&state, installed).await?))
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

    Ok(Json(plugin_item_from_storage(&state, record).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdatePluginSettingsRequest {
    pub enabled: bool,
    pub run_mode: RunMode,
    pub priority: i64,
}

pub async fn update(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    Json(request): Json<UpdatePluginSettingsRequest>,
) -> ApiResult<PluginItem> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    let record = state
        .plugin_manager
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id,
            enabled: request.enabled,
            run_mode: request.run_mode,
            priority: request.priority,
        })
        .await
        .map_err(management_error)?;
    Ok(Json(plugin_item_from_manager(&state, record).await?))
}

pub async fn uninstall(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    state
        .plugin_manager
        .uninstall(&plugin_id)
        .await
        .map_err(management_error)?;
    Ok(StatusCode::NO_CONTENT)
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

fn management_error(error: PluginManagementError) -> (StatusCode, Json<ApiError>) {
    match error {
        PluginManagementError::PluginNotFound => api_error(
            StatusCode::NOT_FOUND,
            "PLUGIN_NOT_FOUND",
            "Plugin is not installed",
        ),
        PluginManagementError::InvalidPriority => api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PRIORITY",
            "Plugin priority must be between 0 and 1000",
        ),
        PluginManagementError::PluginOperationInProgress => api_error(
            StatusCode::CONFLICT,
            "PLUGIN_OPERATION_IN_PROGRESS",
            "Another operation is already running for this plugin",
        ),
        PluginManagementError::PluginDisabled => api_error(
            StatusCode::CONFLICT,
            "PLUGIN_DISABLED",
            "Disabled plugins cannot be started",
        ),
        PluginManagementError::RuntimeUnavailable => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SUPERVISOR_UNAVAILABLE",
            "Plugin management service is unavailable",
        ),
        PluginManagementError::InvalidRuntimeState | PluginManagementError::Internal => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PLUGIN_MANAGEMENT_FAILED",
            "Plugin management request failed",
        ),
    }
}

async fn plugin_item_from_storage(
    state: &AppState,
    record: PluginRecord,
) -> Result<PluginItem, (StatusCode, Json<ApiError>)> {
    let capabilities = manifest_capabilities(&record.manifest_json);
    let (search_enabled, discover_enabled, is_default_content_plugin) = content_routing_metadata(
        state,
        &record.plugin_id,
        record.plugin_type,
        &record.platform_id,
    )
    .await?;
    Ok(PluginItem {
        plugin_id: record.plugin_id.to_string(),
        plugin_type: record.plugin_type,
        platform_id: record.platform_id,
        name: record.name,
        version: record.version,
        status: record.status,
        enabled: record.enabled,
        run_mode: record.run_mode,
        priority: record.priority,
        source_url: record.source_ref,
        commit_sha: record.commit_sha.unwrap_or_default(),
        capabilities,
        search_enabled,
        discover_enabled,
        is_default_content_plugin,
    })
}

async fn plugin_item_from_manager(
    state: &AppState,
    record: InstallPluginRecord,
) -> Result<PluginItem, (StatusCode, Json<ApiError>)> {
    let capabilities = manifest_capabilities(&record.manifest_json);
    let (search_enabled, discover_enabled, is_default_content_plugin) = content_routing_metadata(
        state,
        &record.plugin_id,
        record.plugin_type,
        &record.platform_id,
    )
    .await?;
    Ok(PluginItem {
        plugin_id: record.plugin_id.to_string(),
        plugin_type: record.plugin_type,
        platform_id: record.platform_id,
        name: record.name,
        version: record.version,
        status: record.status,
        enabled: record.enabled,
        run_mode: record.run_mode,
        priority: record.priority,
        source_url: record.source_ref,
        commit_sha: record.commit_sha,
        capabilities,
        search_enabled,
        discover_enabled,
        is_default_content_plugin,
    })
}

fn manifest_capabilities(manifest_json: &serde_json::Value) -> Vec<String> {
    manifest_json
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .map(|capabilities| {
            capabilities
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn content_routing_metadata(
    state: &AppState,
    plugin_id: &PluginId,
    plugin_type: audiodown_plugin_api::manifest::PluginType,
    platform_id: &str,
) -> Result<(Option<bool>, Option<bool>, bool), (StatusCode, Json<ApiError>)> {
    if plugin_type != audiodown_plugin_api::manifest::PluginType::Content {
        return Ok((None, None, false));
    }
    let participation = state
        .storage
        .content_routing()
        .participation(plugin_id)
        .await
        .map_err(internal_error)?;
    let is_default = state
        .storage
        .content_routing()
        .default_for_platform(platform_id)
        .await
        .map_err(internal_error)?
        .is_some_and(|default| default == *plugin_id);
    Ok((
        Some(participation.search_enabled),
        Some(participation.discover_enabled),
        is_default,
    ))
}

fn runtime_state(record: &InstallPluginRecord) -> PluginRuntimeState {
    PluginRuntimeState {
        plugin_id: record.plugin_id.clone(),
        status: record.status,
        container_id: None,
        logs: Vec::new(),
    }
}
