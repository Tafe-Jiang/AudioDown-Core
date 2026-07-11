use std::{collections::BTreeMap, sync::Arc, time::Instant};

use async_trait::async_trait;
use audiodown_content::{
    AggregatedCategoriesResult, AggregatedDiscoverResult, AggregatedSearchResult,
    CategoriesAggregationInput, ContentAggregationService, ContentCandidate, ContentFailure,
    ContentInvokeError, ContentPluginInvoker, ContentRouteKind, ContentRoutingStore,
    ContentServiceError, ContentSource, DiscoverAggregationInput, SearchAggregationInput,
};
use audiodown_domain::{
    log::{LogLevel, StructuredLog},
    plugin::PluginId,
};
use audiodown_plugin_api::{
    content::{
        AlbumGetRequest, AlbumGetResult, ContentMethod, TracksListRequest, TracksListResult,
    },
    error::{PluginErrorCode, PluginErrorData},
    manifest::PluginType,
    rpc::JsonRpcResponse,
};
use audiodown_plugin_manager::service::{
    ContentInvocationError, ContentInvocationRequest, PluginManagerService,
};
use audiodown_storage::{ContentParticipationKind, PluginRecord, Storage};
use chrono::Utc;
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone)]
pub struct ContentApiService {
    storage: Storage,
    plugin_manager: Arc<PluginManagerService>,
}

impl ContentApiService {
    pub fn new(storage: Storage, plugin_manager: Arc<PluginManagerService>) -> Self {
        Self {
            storage,
            plugin_manager,
        }
    }

    pub async fn search(
        &self,
        input: SearchAggregationInput,
    ) -> Result<AggregatedSearchResult, ContentApiError> {
        let request_id = input.request_id.clone();
        let started = Instant::now();
        let result = self.aggregation().search(input).await?;
        self.log_search(&request_id, started, &result).await?;
        Ok(result)
    }

    pub async fn discover(
        &self,
        input: DiscoverAggregationInput,
    ) -> Result<AggregatedDiscoverResult, ContentApiError> {
        let request_id = input.request_id.clone();
        let started = Instant::now();
        let result = self.aggregation().discover(input).await?;
        self.log_discover(&request_id, started, &result).await?;
        Ok(result)
    }

    pub async fn categories(
        &self,
        input: CategoriesAggregationInput,
    ) -> Result<AggregatedCategoriesResult, ContentApiError> {
        let request_id = input.request_id.clone();
        let started = Instant::now();
        let result = self.aggregation().categories(input).await?;
        self.log_categories(&request_id, started, &result).await?;
        Ok(result)
    }

    pub async fn album_get(
        &self,
        request_id: &str,
        plugin_id: &PluginId,
        request: AlbumGetRequest,
    ) -> Result<SourceBoundAlbum, ContentApiError> {
        let started = Instant::now();
        let (source, result) = self
            .invoke_source_bound::<_, AlbumGetResult>(
                request_id,
                plugin_id,
                ContentMethod::AlbumGet,
                request,
            )
            .await?;
        let result = match result {
            Ok(result) => {
                self.append_completion_log(
                    request_id,
                    ContentMethod::AlbumGet,
                    &source,
                    elapsed_millis(started),
                    1,
                    None,
                )
                .await?;
                result
            }
            Err(error) => {
                self.append_completion_log(
                    request_id,
                    ContentMethod::AlbumGet,
                    &source,
                    elapsed_millis(started),
                    0,
                    response_error_code(&error),
                )
                .await?;
                return Err(error);
            }
        };
        Ok(SourceBoundAlbum {
            album: result.album,
            source,
        })
    }

    pub async fn tracks_list(
        &self,
        request_id: &str,
        plugin_id: &PluginId,
        request: TracksListRequest,
    ) -> Result<SourceBoundTracks, ContentApiError> {
        let started = Instant::now();
        let (source, result) = self
            .invoke_source_bound::<_, TracksListResult>(
                request_id,
                plugin_id,
                ContentMethod::TracksList,
                request,
            )
            .await?;
        let result = match result {
            Ok(result) => {
                self.append_completion_log(
                    request_id,
                    ContentMethod::TracksList,
                    &source,
                    elapsed_millis(started),
                    result.items.len(),
                    None,
                )
                .await?;
                result
            }
            Err(error) => {
                self.append_completion_log(
                    request_id,
                    ContentMethod::TracksList,
                    &source,
                    elapsed_millis(started),
                    0,
                    response_error_code(&error),
                )
                .await?;
                return Err(error);
            }
        };
        Ok(SourceBoundTracks {
            items: result.items,
            next_cursor: result.next_cursor,
            source,
        })
    }

    fn aggregation(
        &self,
    ) -> ContentAggregationService<SqliteContentRoutingStore, PluginManagerContentInvoker> {
        ContentAggregationService::new(
            Arc::new(SqliteContentRoutingStore::new(self.storage.clone())),
            Arc::new(PluginManagerContentInvoker::new(Arc::clone(
                &self.plugin_manager,
            ))),
        )
    }

    async fn invoke_source_bound<T, R>(
        &self,
        request_id: &str,
        plugin_id: &PluginId,
        method: ContentMethod,
        request: T,
    ) -> Result<(ContentSource, Result<R, ContentApiError>), ContentApiError>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned + ValidatedContentResult,
    {
        let record = self
            .storage
            .plugins()
            .get(plugin_id)
            .await?
            .ok_or(ContentApiError::PluginNotFound)?;
        validate_source_bound_record(&record, method)?;
        let response = self
            .plugin_manager
            .invoke_content(ContentInvocationRequest {
                request_id: request_id.to_string(),
                plugin_id: plugin_id.clone(),
                method,
                params: serde_json::to_value(request).map_err(|_| ContentApiError::Internal)?,
            })
            .await
            .map_err(ContentApiError::Invocation)?;
        let result = decode_typed_response::<R>(response.response);
        Ok((source_from_record(&record), result))
    }

    async fn log_search(
        &self,
        request_id: &str,
        started: Instant,
        result: &AggregatedSearchResult,
    ) -> Result<(), ContentApiError> {
        let mut counts = BTreeMap::<String, (ContentSource, usize)>::new();
        for item in &result.items {
            let entry = counts
                .entry(item.source.plugin_id.to_string())
                .or_insert_with(|| (item.source.clone(), 0));
            entry.1 += 1;
        }
        self.log_aggregate(
            request_id,
            ContentMethod::Search,
            elapsed_millis(started),
            counts.into_values(),
            &result.failures,
        )
        .await
    }

    async fn log_discover(
        &self,
        request_id: &str,
        started: Instant,
        result: &AggregatedDiscoverResult,
    ) -> Result<(), ContentApiError> {
        let mut counts = BTreeMap::<String, (ContentSource, usize)>::new();
        for section in &result.sections {
            let entry = counts
                .entry(section.source.plugin_id.to_string())
                .or_insert_with(|| (section.source.clone(), 0));
            entry.1 += section.section.items.len();
        }
        self.log_aggregate(
            request_id,
            ContentMethod::Discover,
            elapsed_millis(started),
            counts.into_values(),
            &result.failures,
        )
        .await
    }

    async fn log_categories(
        &self,
        request_id: &str,
        started: Instant,
        result: &AggregatedCategoriesResult,
    ) -> Result<(), ContentApiError> {
        let mut counts = BTreeMap::<String, (ContentSource, usize)>::new();
        for item in &result.items {
            let entry = counts
                .entry(item.source.plugin_id.to_string())
                .or_insert_with(|| (item.source.clone(), 0));
            entry.1 += 1;
        }
        self.log_aggregate(
            request_id,
            ContentMethod::Categories,
            elapsed_millis(started),
            counts.into_values(),
            &result.failures,
        )
        .await
    }

    async fn log_aggregate(
        &self,
        request_id: &str,
        method: ContentMethod,
        duration_ms: u64,
        successes: impl IntoIterator<Item = (ContentSource, usize)>,
        failures: &[ContentFailure],
    ) -> Result<(), ContentApiError> {
        for (source, result_count) in successes {
            self.append_completion_log(
                request_id,
                method,
                &source,
                duration_ms,
                result_count,
                None,
            )
            .await?;
        }
        for failure in failures {
            self.append_completion_log(
                request_id,
                method,
                &failure.source,
                duration_ms,
                0,
                Some(failure.code),
            )
            .await?;
        }
        Ok(())
    }

    async fn append_completion_log(
        &self,
        request_id: &str,
        method: ContentMethod,
        source: &ContentSource,
        duration_ms: u64,
        result_count: usize,
        error_code: Option<PluginErrorCode>,
    ) -> Result<(), ContentApiError> {
        self.storage
            .logs()
            .append(&StructuredLog {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                level: if error_code.is_some() {
                    LogLevel::Warn
                } else {
                    LogLevel::Info
                },
                component: "content-api".to_string(),
                message: if error_code.is_some() {
                    "Content call completed with a safe plugin error".to_string()
                } else {
                    "Content call completed".to_string()
                },
                plugin_id: Some(source.plugin_id.to_string()),
                plugin_version: Some(source.plugin_version.clone()),
                platform_id: Some(source.platform_id.clone()),
                request_id: Some(request_id.to_string()),
                task_id: None,
                container_id: None,
                error_code: error_code.map(error_code_name).map(str::to_string),
                context: serde_json::json!({
                    "method": method.capability(),
                    "durationMs": duration_ms,
                    "resultCount": result_count,
                }),
            })
            .await?;
        Ok(())
    }
}

pub struct SqliteContentRoutingStore {
    storage: Storage,
}

impl SqliteContentRoutingStore {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl ContentRoutingStore for SqliteContentRoutingStore {
    async fn list_candidates(
        &self,
        route: ContentRouteKind,
    ) -> Result<Vec<ContentCandidate>, ContentServiceError> {
        let participation = match route {
            ContentRouteKind::Search => ContentParticipationKind::Search,
            ContentRouteKind::Discover => ContentParticipationKind::Discover,
        };
        self.storage
            .content_routing()
            .list_candidates(participation, None, None)
            .await
            .map_err(|error| ContentServiceError::Routing(error.to_string()))?
            .into_iter()
            .map(|candidate| {
                let capabilities = manifest_capabilities(&candidate.manifest_json);
                Ok(ContentCandidate {
                    plugin_id: candidate.plugin_id,
                    plugin_name: candidate.name,
                    plugin_version: candidate.version,
                    platform_id: candidate.platform_id,
                    priority: candidate.priority,
                    is_default: candidate.is_default,
                    search_enabled: true,
                    discover_enabled: true,
                    capabilities,
                })
            })
            .collect()
    }
}

pub struct PluginManagerContentInvoker {
    plugin_manager: Arc<PluginManagerService>,
}

impl PluginManagerContentInvoker {
    pub fn new(plugin_manager: Arc<PluginManagerService>) -> Self {
        Self { plugin_manager }
    }
}

#[async_trait]
impl ContentPluginInvoker for PluginManagerContentInvoker {
    async fn invoke(
        &self,
        request_id: &str,
        candidate: &ContentCandidate,
        method: ContentMethod,
        params: serde_json::Value,
    ) -> Result<JsonRpcResponse, ContentInvokeError> {
        self.plugin_manager
            .invoke_content(ContentInvocationRequest {
                request_id: request_id.to_string(),
                plugin_id: candidate.plugin_id.clone(),
                method,
                params,
            })
            .await
            .map(|result| result.response)
            .map_err(map_invocation_error)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBoundAlbum {
    pub album: audiodown_plugin_api::content::AlbumDetail,
    pub source: ContentSource,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBoundTracks {
    pub items: Vec<audiodown_plugin_api::content::TrackItem>,
    pub next_cursor: Option<String>,
    pub source: ContentSource,
}

#[derive(Debug, Error)]
pub enum ContentApiError {
    #[error("content service failed: {0}")]
    Service(#[from] ContentServiceError),
    #[error("plugin was not found")]
    PluginNotFound,
    #[error("plugin cannot handle this content request")]
    CapabilityMissing,
    #[error("plugin invocation failed: {0}")]
    Invocation(ContentInvocationError),
    #[error("plugin returned {code:?}: {summary}")]
    Plugin {
        code: PluginErrorCode,
        summary: String,
    },
    #[error("plugin response was invalid")]
    InvalidResponse,
    #[error("content service failed")]
    Internal,
    #[error("storage failed: {0}")]
    Storage(#[from] audiodown_storage::StorageError),
}

trait ValidatedContentResult {
    fn validate_result(&self) -> bool;
}

impl ValidatedContentResult for AlbumGetResult {
    fn validate_result(&self) -> bool {
        self.validate().is_ok()
    }
}

impl ValidatedContentResult for TracksListResult {
    fn validate_result(&self) -> bool {
        self.validate().is_ok()
    }
}

fn decode_typed_response<R>(response: JsonRpcResponse) -> Result<R, ContentApiError>
where
    R: serde::de::DeserializeOwned + ValidatedContentResult,
{
    if response.jsonrpc != "2.0" || response.result.is_some() == response.error.is_some() {
        return Err(ContentApiError::InvalidResponse);
    }
    if let Some(error) = response.error {
        let data = error
            .data
            .and_then(|value| serde_json::from_value::<PluginErrorData>(value).ok())
            .filter(|data| data.validate().is_ok())
            .ok_or(ContentApiError::InvalidResponse)?;
        return Err(ContentApiError::Plugin {
            code: data.code,
            summary: safe_plugin_summary(data.code).to_string(),
        });
    }
    let result = response
        .result
        .and_then(|value| serde_json::from_value::<R>(value).ok())
        .ok_or(ContentApiError::InvalidResponse)?;
    if !result.validate_result() {
        return Err(ContentApiError::InvalidResponse);
    }
    Ok(result)
}

fn validate_source_bound_record(
    record: &PluginRecord,
    method: ContentMethod,
) -> Result<(), ContentApiError> {
    if record.plugin_type != PluginType::Content || !record.enabled {
        return Err(ContentApiError::CapabilityMissing);
    }
    if !manifest_capabilities(&record.manifest_json).contains(&method) {
        return Err(ContentApiError::CapabilityMissing);
    }
    Ok(())
}

fn manifest_capabilities(manifest: &serde_json::Value) -> Vec<ContentMethod> {
    manifest
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(ContentMethod::from_capability)
        .collect()
}

fn source_from_record(record: &PluginRecord) -> ContentSource {
    ContentSource {
        plugin_id: record.plugin_id.clone(),
        plugin_name: record.name.clone(),
        plugin_version: record.version.clone(),
        platform_id: record.platform_id.clone(),
    }
}

fn map_invocation_error(error: ContentInvocationError) -> ContentInvokeError {
    match error {
        ContentInvocationError::RuntimeUnavailable | ContentInvocationError::PluginBusy => {
            ContentInvokeError::Unavailable
        }
        ContentInvocationError::InvalidResponse => ContentInvokeError::InvalidResponse,
        _ => ContentInvokeError::Internal,
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn response_error_code(error: &ContentApiError) -> Option<PluginErrorCode> {
    match error {
        ContentApiError::Plugin { code, .. } => Some(*code),
        ContentApiError::InvalidResponse => Some(PluginErrorCode::PluginResponseInvalid),
        _ => None,
    }
}

const fn safe_plugin_summary(code: PluginErrorCode) -> &'static str {
    match code {
        PluginErrorCode::InvalidRequest => "Content request was rejected",
        PluginErrorCode::PluginNotFound => "Content plugin was not found",
        PluginErrorCode::PluginDisabled => "Content plugin is disabled",
        PluginErrorCode::PluginCapabilityMissing => "Content capability is unavailable",
        PluginErrorCode::PluginUnavailable => "Content plugin is unavailable",
        PluginErrorCode::PluginTimeout => "Content plugin timed out",
        PluginErrorCode::PluginResponseInvalid => "Content plugin response was invalid",
        PluginErrorCode::ResourceNotFound => "Content resource was not found",
        PluginErrorCode::ResourceAccessDenied => "Content resource access was denied",
        PluginErrorCode::ResourceTemporarilyUnavailable => {
            "Content resource is temporarily unavailable"
        }
        PluginErrorCode::RateLimited => "Content source is rate limited",
        PluginErrorCode::PlatformResponseChanged => "Content source response changed",
        PluginErrorCode::PluginInternalError => "Content plugin failed",
    }
}

pub const fn error_code_name(code: PluginErrorCode) -> &'static str {
    match code {
        PluginErrorCode::InvalidRequest => "INVALID_REQUEST",
        PluginErrorCode::PluginNotFound => "PLUGIN_NOT_FOUND",
        PluginErrorCode::PluginDisabled => "PLUGIN_DISABLED",
        PluginErrorCode::PluginCapabilityMissing => "PLUGIN_CAPABILITY_MISSING",
        PluginErrorCode::PluginUnavailable => "PLUGIN_UNAVAILABLE",
        PluginErrorCode::PluginTimeout => "PLUGIN_TIMEOUT",
        PluginErrorCode::PluginResponseInvalid => "PLUGIN_RESPONSE_INVALID",
        PluginErrorCode::ResourceNotFound => "RESOURCE_NOT_FOUND",
        PluginErrorCode::ResourceAccessDenied => "RESOURCE_ACCESS_DENIED",
        PluginErrorCode::ResourceTemporarilyUnavailable => "RESOURCE_TEMPORARILY_UNAVAILABLE",
        PluginErrorCode::RateLimited => "RATE_LIMITED",
        PluginErrorCode::PlatformResponseChanged => "PLATFORM_RESPONSE_CHANGED",
        PluginErrorCode::PluginInternalError => "PLUGIN_INTERNAL_ERROR",
    }
}
