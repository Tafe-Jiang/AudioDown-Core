use audiodown_content::{
    CategoriesAggregationInput, ContentFailure, ContentFilters, DiscoverAggregationInput,
    SearchAggregationInput, SourcedCategoryItem, SourcedContentItem, SourcedDiscoverSection,
};
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::content::{
    AlbumGetRequest, DiscoverRequest, SearchRequest, TracksListRequest,
};
use audiodown_storage::{ContentParticipation, StorageError};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    content_adapters::{error_code_name, ContentApiError, SourceBoundAlbum, SourceBoundTracks},
    routes::{internal_error, ApiError, ApiResult},
    state::AppState,
};

const DEFAULT_LIMIT: u16 = 20;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub q: Option<String>,
    pub platform_id: Option<String>,
    pub plugin_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverQuery {
    pub platform_id: Option<String>,
    pub plugin_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoriesQuery {
    pub platform_id: Option<String>,
    pub plugin_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentCollectionResponse {
    pub items: Vec<SourcedContentItem>,
    pub sections: Vec<SourcedDiscoverSection>,
    pub next_cursor: Option<String>,
    pub failures: Vec<ContentFailure>,
    pub empty_state: Option<EmptyStateResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoriesResponse {
    pub items: Vec<SourcedCategoryItem>,
    pub failures: Vec<ContentFailure>,
    pub empty_state: Option<EmptyStateResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyStateResponse {
    pub reason: &'static str,
    pub title: &'static str,
    pub action_label: &'static str,
}

pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> ApiResult<ContentCollectionResponse> {
    let request_id = request_id(&headers);
    let query_text = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    SearchRequest {
        query: query_text.clone(),
        cursor: None,
        limit,
    }
    .validate()
    .map_err(|_| invalid_request("INVALID_SEARCH_QUERY", "Search query or limit is invalid"))?;
    let filters = parse_filters(query.platform_id, query.plugin_id)?;
    let result = state
        .content
        .search(SearchAggregationInput {
            request_id,
            query: query_text,
            limit,
            filters,
            cursor: query.cursor,
        })
        .await
        .map_err(content_error)?;
    Ok(Json(ContentCollectionResponse {
        items: result.items,
        sections: Vec::new(),
        next_cursor: result.next_cursor,
        failures: result.failures,
        empty_state: (!result.had_candidates).then(empty_state),
    }))
}

pub async fn discover(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DiscoverQuery>,
) -> ApiResult<ContentCollectionResponse> {
    let request_id = request_id(&headers);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    DiscoverRequest {
        cursor: None,
        limit,
    }
    .validate()
    .map_err(|_| invalid_request("INVALID_DISCOVER_QUERY", "Discover limit is invalid"))?;
    let filters = parse_filters(query.platform_id, query.plugin_id)?;
    let result = state
        .content
        .discover(DiscoverAggregationInput {
            request_id,
            limit,
            filters,
            cursor: query.cursor,
        })
        .await
        .map_err(content_error)?;
    Ok(Json(ContentCollectionResponse {
        items: Vec::new(),
        sections: result.sections,
        next_cursor: result.next_cursor,
        failures: result.failures,
        empty_state: (!result.had_candidates).then(empty_state),
    }))
}

pub async fn categories(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CategoriesQuery>,
) -> ApiResult<CategoriesResponse> {
    let result = state
        .content
        .categories(CategoriesAggregationInput {
            request_id: request_id(&headers),
            filters: parse_filters(query.platform_id, query.plugin_id)?,
        })
        .await
        .map_err(content_error)?;
    Ok(Json(CategoriesResponse {
        items: result.items,
        failures: result.failures,
        empty_state: (!result.had_candidates).then(empty_state),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlbumRequest {
    pub plugin_id: String,
    pub resource_id: String,
}

pub async fn album_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AlbumRequest>,
) -> ApiResult<SourceBoundAlbum> {
    let plugin_id = parse_plugin_id(request.plugin_id)?;
    let plugin_request = AlbumGetRequest {
        resource_id: request.resource_id,
    };
    plugin_request
        .validate()
        .map_err(|_| invalid_request("INVALID_ALBUM_REQUEST", "Album request is invalid"))?;
    state
        .content
        .album_get(&request_id(&headers), &plugin_id, plugin_request)
        .await
        .map(Json)
        .map_err(content_error)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TracksRequest {
    pub plugin_id: String,
    pub album_resource_id: String,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

pub async fn tracks_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TracksRequest>,
) -> ApiResult<SourceBoundTracks> {
    let plugin_id = parse_plugin_id(request.plugin_id)?;
    let plugin_request = TracksListRequest {
        album_resource_id: request.album_resource_id,
        cursor: request.cursor,
        limit: request.limit.unwrap_or(DEFAULT_LIMIT),
    };
    plugin_request
        .validate()
        .map_err(|_| invalid_request("INVALID_TRACKS_REQUEST", "Tracks request is invalid"))?;
    state
        .content
        .tracks_list(&request_id(&headers), &plugin_id, plugin_request)
        .await
        .map(Json)
        .map_err(content_error)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentSettingsRequest {
    pub search_enabled: bool,
    pub discover_enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSettingsResponse {
    pub plugin_id: PluginId,
    pub search_enabled: bool,
    pub discover_enabled: bool,
}

pub async fn update_settings(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    Json(request): Json<ContentSettingsRequest>,
) -> ApiResult<ContentSettingsResponse> {
    let plugin_id = parse_plugin_id(plugin_id)?;
    state
        .storage
        .content_routing()
        .update_participation(
            &plugin_id,
            ContentParticipation {
                search_enabled: request.search_enabled,
                discover_enabled: request.discover_enabled,
            },
        )
        .await
        .map_err(settings_storage_error)?;
    Ok(Json(ContentSettingsResponse {
        plugin_id,
        search_enabled: request.search_enabled,
        discover_enabled: request.discover_enabled,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefaultPluginRequest {
    pub plugin_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultPluginResponse {
    pub platform_id: String,
    pub plugin_id: PluginId,
}

pub async fn set_default(
    State(state): State<AppState>,
    Path(platform_id): Path<String>,
    Json(request): Json<DefaultPluginRequest>,
) -> ApiResult<DefaultPluginResponse> {
    let plugin_id = parse_plugin_id(request.plugin_id)?;
    state
        .storage
        .content_routing()
        .set_default(&platform_id, &plugin_id)
        .await
        .map_err(default_storage_error)?;
    Ok(Json(DefaultPluginResponse {
        platform_id,
        plugin_id,
    }))
}

fn parse_filters(
    platform_id: Option<String>,
    plugin_id: Option<String>,
) -> Result<ContentFilters, (StatusCode, Json<ApiError>)> {
    if platform_id
        .as_ref()
        .is_some_and(|value| !valid_platform_id(value))
    {
        return Err(invalid_request(
            "INVALID_PLATFORM_ID",
            "Platform ID is invalid",
        ));
    }
    Ok(ContentFilters {
        platform_id,
        plugin_id: plugin_id.map(parse_plugin_id).transpose()?,
    })
}

fn parse_plugin_id(plugin_id: String) -> Result<PluginId, (StatusCode, Json<ApiError>)> {
    PluginId::parse(plugin_id)
        .map_err(|_| invalid_request("INVALID_PLUGIN_ID", "Plugin ID is invalid"))
}

fn valid_platform_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 128 && !value.contains('\0'))
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn empty_state() -> EmptyStateResponse {
    EmptyStateResponse {
        reason: "NO_CONTENT_PLUGINS",
        title: "尚未安装内容插件",
        action_label: "添加 GitHub 插件仓库",
    }
}

fn content_error(error: ContentApiError) -> (StatusCode, Json<ApiError>) {
    match error {
        ContentApiError::Service(audiodown_content::ContentServiceError::Cursor(_)) => {
            invalid_request("INVALID_CURSOR", "Content cursor is invalid")
        }
        ContentApiError::PluginNotFound => api_error(
            StatusCode::NOT_FOUND,
            "CONTENT_PLUGIN_NOT_FOUND",
            "Content plugin was not found",
        ),
        ContentApiError::CapabilityMissing => api_error(
            StatusCode::CONFLICT,
            "PLUGIN_CAPABILITY_MISSING",
            "Plugin cannot handle this content request",
        ),
        ContentApiError::Plugin { code, summary } => {
            let status = if code == audiodown_plugin_api::error::PluginErrorCode::ResourceNotFound {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            };
            api_error(status, error_code_name(code), &summary)
        }
        ContentApiError::Invocation(error) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            error.standard_code(),
            "Content plugin is unavailable",
        ),
        ContentApiError::InvalidResponse => api_error(
            StatusCode::BAD_GATEWAY,
            "PLUGIN_RESPONSE_INVALID",
            "Content plugin returned an invalid response",
        ),
        other => internal_error(other),
    }
}

fn settings_storage_error(error: StorageError) -> (StatusCode, Json<ApiError>) {
    match error {
        StorageError::NotFound => api_error(
            StatusCode::NOT_FOUND,
            "CONTENT_PLUGIN_NOT_FOUND",
            "Content plugin was not found",
        ),
        StorageError::InvalidData(_) => {
            invalid_request("INVALID_CONTENT_SETTINGS", "Content settings are invalid")
        }
        other => internal_error(other),
    }
}

fn default_storage_error(error: StorageError) -> (StatusCode, Json<ApiError>) {
    match error {
        StorageError::NotFound => api_error(
            StatusCode::NOT_FOUND,
            "CONTENT_PLUGIN_NOT_FOUND",
            "Content plugin was not found",
        ),
        StorageError::InvalidData(_) => invalid_request(
            "INVALID_CONTENT_DEFAULT",
            "Default content plugin is invalid",
        ),
        other => internal_error(other),
    }
}

fn invalid_request(code: &'static str, message: &str) -> (StatusCode, Json<ApiError>) {
    api_error(StatusCode::BAD_REQUEST, code, message)
}

fn api_error(
    status: StatusCode,
    code: &'static str,
    message: &str,
) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            code,
            message: message.to_string(),
        }),
    )
}
