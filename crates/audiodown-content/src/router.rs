use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::{
    content::{
        CategoriesRequest, CategoriesResult, CategoryItem, ContentItem, ContentMethod,
        DiscoverRequest, DiscoverResult, DiscoverSection, SearchRequest, SearchResult,
    },
    error::{PluginErrorCode, PluginErrorData},
    rpc::JsonRpcResponse,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    decode_cursor, deduplicate_categories, deduplicate_items, deduplicate_sections, encode_cursor,
    ContentCursorBinding, ContentCursorError, ContentCursorOperation, SourceCursor,
};

const PLATFORM_CONCURRENCY: usize = 4;

#[derive(Debug, Clone)]
pub struct ContentCandidate {
    pub plugin_id: PluginId,
    pub plugin_name: String,
    pub plugin_version: String,
    pub platform_id: String,
    pub priority: i64,
    pub is_default: bool,
    pub search_enabled: bool,
    pub discover_enabled: bool,
    pub capabilities: Vec<ContentMethod>,
}

#[derive(Debug, Clone, Default)]
pub struct ContentFilters {
    pub platform_id: Option<String>,
    pub plugin_id: Option<PluginId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentRouteKind {
    Search,
    Discover,
}

pub fn select_candidates(
    candidates: Vec<ContentCandidate>,
    method: ContentMethod,
    route: ContentRouteKind,
    filters: &ContentFilters,
) -> Vec<ContentCandidate> {
    let mut selected = candidates
        .into_iter()
        .filter(|candidate| {
            let participates = match route {
                ContentRouteKind::Search => candidate.search_enabled,
                ContentRouteKind::Discover => candidate.discover_enabled,
            };
            participates
                && candidate.capabilities.contains(&method)
                && filters
                    .platform_id
                    .as_ref()
                    .is_none_or(|platform_id| candidate.platform_id == *platform_id)
                && filters
                    .plugin_id
                    .as_ref()
                    .is_none_or(|plugin_id| candidate.plugin_id == *plugin_id)
        })
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        left.platform_id
            .cmp(&right.platform_id)
            .then_with(|| right.is_default.cmp(&left.is_default))
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.plugin_id.as_str().cmp(right.plugin_id.as_str()))
    });
    selected
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContentSource {
    pub plugin_id: PluginId,
    pub plugin_name: String,
    pub plugin_version: String,
    pub platform_id: String,
}

impl From<&ContentCandidate> for ContentSource {
    fn from(candidate: &ContentCandidate) -> Self {
        Self {
            plugin_id: candidate.plugin_id.clone(),
            plugin_name: candidate.plugin_name.clone(),
            plugin_version: candidate.plugin_version.clone(),
            platform_id: candidate.platform_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcedContentItem {
    pub item: ContentItem,
    pub source: ContentSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcedDiscoverSection {
    pub section: DiscoverSection,
    pub source: ContentSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentFailure {
    pub source: ContentSource,
    pub code: PluginErrorCode,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedSearchResult {
    pub items: Vec<SourcedContentItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub failures: Vec<ContentFailure>,
    #[serde(skip)]
    pub had_candidates: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedDiscoverResult {
    pub sections: Vec<SourcedDiscoverSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub failures: Vec<ContentFailure>,
    #[serde(skip)]
    pub had_candidates: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcedCategoryItem {
    pub item: CategoryItem,
    pub source: ContentSource,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedCategoriesResult {
    pub items: Vec<SourcedCategoryItem>,
    pub failures: Vec<ContentFailure>,
    #[serde(skip)]
    pub had_candidates: bool,
}

#[derive(Debug, Clone)]
pub struct SearchAggregationInput {
    pub request_id: String,
    pub query: String,
    pub limit: u16,
    pub filters: ContentFilters,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoverAggregationInput {
    pub request_id: String,
    pub limit: u16,
    pub filters: ContentFilters,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CategoriesAggregationInput {
    pub request_id: String,
    pub filters: ContentFilters,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ContentInvokeError {
    #[error("plugin is unavailable")]
    Unavailable,
    #[error("plugin call timed out")]
    Timeout,
    #[error("plugin response was invalid")]
    InvalidResponse,
    #[error("plugin call failed")]
    Internal,
}

#[derive(Debug, Error)]
pub enum ContentServiceError {
    #[error("content routing store failed: {0}")]
    Routing(String),
    #[error("content request could not be serialized")]
    RequestSerialization,
    #[error("content cursor is invalid: {0}")]
    Cursor(#[from] ContentCursorError),
}

#[async_trait]
pub trait ContentRoutingStore: Send + Sync {
    async fn list_candidates(
        &self,
        route: ContentRouteKind,
    ) -> Result<Vec<ContentCandidate>, ContentServiceError>;
}

#[async_trait]
pub trait ContentPluginInvoker: Send + Sync {
    async fn invoke(
        &self,
        request_id: &str,
        candidate: &ContentCandidate,
        method: ContentMethod,
        params: serde_json::Value,
    ) -> Result<JsonRpcResponse, ContentInvokeError>;
}

pub struct ContentAggregationService<S, I> {
    store: Arc<S>,
    invoker: Arc<I>,
}

impl<S, I> ContentAggregationService<S, I>
where
    S: ContentRoutingStore + 'static,
    I: ContentPluginInvoker + 'static,
{
    pub fn new(store: Arc<S>, invoker: Arc<I>) -> Self {
        Self { store, invoker }
    }

    pub async fn search(
        &self,
        input: SearchAggregationInput,
    ) -> Result<AggregatedSearchResult, ContentServiceError> {
        let binding = ContentCursorBinding {
            operation: ContentCursorOperation::Search,
            query: Some(input.query.clone()),
            filters: input.filters.clone(),
        };
        let candidates = self.store.list_candidates(ContentRouteKind::Search).await?;
        let selected = select_candidates(
            candidates,
            ContentMethod::Search,
            ContentRouteKind::Search,
            &input.filters,
        );
        let (groups, allow_fallback) = prepare_groups(selected, input.cursor.as_deref(), &binding)?;
        let had_candidates = !groups.is_empty();
        let work = groups
            .into_iter()
            .map(|group| {
                serde_json::to_value(SearchRequest {
                    query: input.query.clone(),
                    cursor: group.cursor,
                    limit: input.limit,
                })
                .map(|params| (group.candidates, params))
                .map_err(|_| ContentServiceError::RequestSerialization)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let invoker = Arc::clone(&self.invoker);
        let request_id = input.request_id;
        let mut platform_results = stream::iter(work.into_iter().enumerate())
            .map(|(index, candidates)| {
                let invoker = Arc::clone(&invoker);
                let request_id = request_id.clone();
                let (candidates, params) = candidates;
                async move {
                    let result = search_platform(
                        invoker.as_ref(),
                        &request_id,
                        candidates,
                        params,
                        allow_fallback,
                    )
                    .await;
                    (index, result)
                }
            })
            .buffer_unordered(PLATFORM_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        platform_results.sort_by_key(|(index, _)| *index);

        let mut aggregate = AggregatedSearchResult::default();
        let mut next_sources = Vec::new();
        for (_, result) in platform_results {
            aggregate.items.extend(result.aggregate.items);
            aggregate.failures.extend(result.aggregate.failures);
            if let Some(source_cursor) = result.next_cursor {
                next_sources.push(source_cursor);
            }
        }
        aggregate.next_cursor = encode_cursor(&binding, &next_sources)?;
        aggregate.had_candidates = had_candidates;
        aggregate.items = deduplicate_items(aggregate.items);
        Ok(aggregate)
    }

    pub async fn discover(
        &self,
        input: DiscoverAggregationInput,
    ) -> Result<AggregatedDiscoverResult, ContentServiceError> {
        let binding = ContentCursorBinding {
            operation: ContentCursorOperation::Discover,
            query: None,
            filters: input.filters.clone(),
        };
        let candidates = self
            .store
            .list_candidates(ContentRouteKind::Discover)
            .await?;
        let selected = select_candidates(
            candidates,
            ContentMethod::Discover,
            ContentRouteKind::Discover,
            &input.filters,
        );
        let (groups, allow_fallback) = prepare_groups(selected, input.cursor.as_deref(), &binding)?;
        let had_candidates = !groups.is_empty();
        let work = groups
            .into_iter()
            .map(|group| {
                serde_json::to_value(DiscoverRequest {
                    cursor: group.cursor,
                    limit: input.limit,
                })
                .map(|params| (group.candidates, params))
                .map_err(|_| ContentServiceError::RequestSerialization)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let invoker = Arc::clone(&self.invoker);
        let request_id = input.request_id;
        let mut platform_results = stream::iter(work.into_iter().enumerate())
            .map(|(index, candidates)| {
                let invoker = Arc::clone(&invoker);
                let request_id = request_id.clone();
                let (candidates, params) = candidates;
                async move {
                    let result = discover_platform(
                        invoker.as_ref(),
                        &request_id,
                        candidates,
                        params,
                        allow_fallback,
                    )
                    .await;
                    (index, result)
                }
            })
            .buffer_unordered(PLATFORM_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        platform_results.sort_by_key(|(index, _)| *index);

        let mut aggregate = AggregatedDiscoverResult::default();
        let mut next_sources = Vec::new();
        for (_, result) in platform_results {
            aggregate.sections.extend(result.aggregate.sections);
            aggregate.failures.extend(result.aggregate.failures);
            if let Some(source_cursor) = result.next_cursor {
                next_sources.push(source_cursor);
            }
        }
        aggregate.next_cursor = encode_cursor(&binding, &next_sources)?;
        aggregate.had_candidates = had_candidates;
        deduplicate_sections(&mut aggregate.sections);
        Ok(aggregate)
    }

    pub async fn categories(
        &self,
        input: CategoriesAggregationInput,
    ) -> Result<AggregatedCategoriesResult, ContentServiceError> {
        let candidates = self
            .store
            .list_candidates(ContentRouteKind::Discover)
            .await?;
        let groups = group_candidates(select_candidates(
            candidates,
            ContentMethod::Categories,
            ContentRouteKind::Discover,
            &input.filters,
        ));
        let had_candidates = !groups.is_empty();
        let params = serde_json::to_value(CategoriesRequest::default())
            .map_err(|_| ContentServiceError::RequestSerialization)?;
        let allow_fallback = input.filters.plugin_id.is_none();
        let invoker = Arc::clone(&self.invoker);
        let request_id = input.request_id;
        let mut platform_results = stream::iter(groups.into_iter().enumerate())
            .map(|(index, candidates)| {
                let invoker = Arc::clone(&invoker);
                let request_id = request_id.clone();
                let params = params.clone();
                async move {
                    let result = categories_platform(
                        invoker.as_ref(),
                        &request_id,
                        candidates,
                        params,
                        allow_fallback,
                    )
                    .await;
                    (index, result)
                }
            })
            .buffer_unordered(PLATFORM_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        platform_results.sort_by_key(|(index, _)| *index);

        let mut aggregate = AggregatedCategoriesResult {
            had_candidates,
            ..Default::default()
        };
        for (_, result) in platform_results {
            aggregate.items.extend(result.items);
            aggregate.failures.extend(result.failures);
        }
        aggregate.items = deduplicate_categories(aggregate.items);
        Ok(aggregate)
    }
}

fn group_candidates(candidates: Vec<ContentCandidate>) -> Vec<Vec<ContentCandidate>> {
    let mut groups = BTreeMap::<String, Vec<ContentCandidate>>::new();
    for candidate in candidates {
        groups
            .entry(candidate.platform_id.clone())
            .or_default()
            .push(candidate);
    }
    groups.into_values().collect()
}

struct CursorCandidateGroup {
    candidates: Vec<ContentCandidate>,
    cursor: Option<String>,
}

fn prepare_groups(
    candidates: Vec<ContentCandidate>,
    cursor: Option<&str>,
    binding: &ContentCursorBinding,
) -> Result<(Vec<CursorCandidateGroup>, bool), ContentServiceError> {
    let Some(cursor) = cursor else {
        return Ok((
            group_candidates(candidates)
                .into_iter()
                .map(|candidates| CursorCandidateGroup {
                    candidates,
                    cursor: None,
                })
                .collect(),
            binding.filters.plugin_id.is_none(),
        ));
    };

    let sources = decode_cursor(cursor, binding)?;
    let mut groups = Vec::with_capacity(sources.len());
    for source in sources {
        let candidate = candidates
            .iter()
            .find(|candidate| {
                candidate.platform_id == source.platform_id
                    && candidate.plugin_id == source.plugin_id
            })
            .cloned()
            .ok_or(ContentCursorError::BindingMismatch)?;
        groups.push(CursorCandidateGroup {
            candidates: vec![candidate],
            cursor: Some(source.cursor),
        });
    }
    Ok((groups, false))
}

struct PlatformSearchResult {
    aggregate: AggregatedSearchResult,
    next_cursor: Option<SourceCursor>,
}

async fn search_platform<I: ContentPluginInvoker>(
    invoker: &I,
    request_id: &str,
    candidates: Vec<ContentCandidate>,
    params: serde_json::Value,
    allow_fallback: bool,
) -> PlatformSearchResult {
    let mut aggregate = AggregatedSearchResult::default();
    let mut next_cursor = None;
    for candidate in candidates {
        match invoker
            .invoke(
                request_id,
                &candidate,
                ContentMethod::Search,
                params.clone(),
            )
            .await
        {
            Ok(response) => match decode_search_response(response) {
                Ok(result) => {
                    let source = ContentSource::from(&candidate);
                    let SearchResult {
                        items,
                        next_cursor: plugin_cursor,
                    } = result;
                    aggregate
                        .items
                        .extend(items.into_iter().map(|item| SourcedContentItem {
                            item,
                            source: source.clone(),
                        }));
                    next_cursor = plugin_cursor.map(|cursor| SourceCursor {
                        platform_id: candidate.platform_id.clone(),
                        plugin_id: candidate.plugin_id.clone(),
                        cursor,
                    });
                    break;
                }
                Err(failure) => {
                    let retryable = failure.code.is_retryable();
                    aggregate
                        .failures
                        .push(failure.with_source(ContentSource::from(&candidate)));
                    if !allow_fallback || !retryable {
                        break;
                    }
                }
            },
            Err(error) => {
                let failure = invoke_failure(error);
                let retryable = failure.code.is_retryable();
                aggregate
                    .failures
                    .push(failure.with_source(ContentSource::from(&candidate)));
                if !allow_fallback || !retryable {
                    break;
                }
            }
        }
    }
    PlatformSearchResult {
        aggregate,
        next_cursor,
    }
}

struct PlatformDiscoverResult {
    aggregate: AggregatedDiscoverResult,
    next_cursor: Option<SourceCursor>,
}

async fn discover_platform<I: ContentPluginInvoker>(
    invoker: &I,
    request_id: &str,
    candidates: Vec<ContentCandidate>,
    params: serde_json::Value,
    allow_fallback: bool,
) -> PlatformDiscoverResult {
    let mut aggregate = AggregatedDiscoverResult::default();
    let mut next_cursor = None;
    for candidate in candidates {
        match invoker
            .invoke(
                request_id,
                &candidate,
                ContentMethod::Discover,
                params.clone(),
            )
            .await
        {
            Ok(response) => match decode_discover_response(response) {
                Ok(result) => {
                    let source = ContentSource::from(&candidate);
                    let DiscoverResult {
                        sections,
                        next_cursor: plugin_cursor,
                    } = result;
                    aggregate
                        .sections
                        .extend(sections.into_iter().map(|section| SourcedDiscoverSection {
                            section,
                            source: source.clone(),
                        }));
                    next_cursor = plugin_cursor.map(|cursor| SourceCursor {
                        platform_id: candidate.platform_id.clone(),
                        plugin_id: candidate.plugin_id.clone(),
                        cursor,
                    });
                    break;
                }
                Err(failure) => {
                    let retryable = failure.code.is_retryable();
                    aggregate
                        .failures
                        .push(failure.with_source(ContentSource::from(&candidate)));
                    if !allow_fallback || !retryable {
                        break;
                    }
                }
            },
            Err(error) => {
                let failure = invoke_failure(error);
                let retryable = failure.code.is_retryable();
                aggregate
                    .failures
                    .push(failure.with_source(ContentSource::from(&candidate)));
                if !allow_fallback || !retryable {
                    break;
                }
            }
        }
    }
    PlatformDiscoverResult {
        aggregate,
        next_cursor,
    }
}

async fn categories_platform<I: ContentPluginInvoker>(
    invoker: &I,
    request_id: &str,
    candidates: Vec<ContentCandidate>,
    params: serde_json::Value,
    allow_fallback: bool,
) -> AggregatedCategoriesResult {
    let mut aggregate = AggregatedCategoriesResult::default();
    for candidate in candidates {
        match invoker
            .invoke(
                request_id,
                &candidate,
                ContentMethod::Categories,
                params.clone(),
            )
            .await
        {
            Ok(response) => match decode_categories_response(response) {
                Ok(result) => {
                    let source = ContentSource::from(&candidate);
                    aggregate.items.extend(result.items.into_iter().map(|item| {
                        SourcedCategoryItem {
                            item,
                            source: source.clone(),
                        }
                    }));
                    break;
                }
                Err(failure) => {
                    let retryable = failure.code.is_retryable();
                    aggregate
                        .failures
                        .push(failure.with_source(ContentSource::from(&candidate)));
                    if !allow_fallback || !retryable {
                        break;
                    }
                }
            },
            Err(error) => {
                let failure = invoke_failure(error);
                let retryable = failure.code.is_retryable();
                aggregate
                    .failures
                    .push(failure.with_source(ContentSource::from(&candidate)));
                if !allow_fallback || !retryable {
                    break;
                }
            }
        }
    }
    aggregate
}

fn decode_search_response(response: JsonRpcResponse) -> Result<SearchResult, FailureData> {
    let value = decode_response(response)?;
    let result = serde_json::from_value::<SearchResult>(value)
        .map_err(|_| FailureData::invalid_response())?;
    result
        .validate()
        .map_err(|_| FailureData::invalid_response())?;
    Ok(result)
}

fn decode_discover_response(response: JsonRpcResponse) -> Result<DiscoverResult, FailureData> {
    let value = decode_response(response)?;
    let result = serde_json::from_value::<DiscoverResult>(value)
        .map_err(|_| FailureData::invalid_response())?;
    result
        .validate()
        .map_err(|_| FailureData::invalid_response())?;
    Ok(result)
}

fn decode_categories_response(response: JsonRpcResponse) -> Result<CategoriesResult, FailureData> {
    let value = decode_response(response)?;
    let result = serde_json::from_value::<CategoriesResult>(value)
        .map_err(|_| FailureData::invalid_response())?;
    result
        .validate()
        .map_err(|_| FailureData::invalid_response())?;
    Ok(result)
}

fn decode_response(response: JsonRpcResponse) -> Result<serde_json::Value, FailureData> {
    if response.jsonrpc != "2.0" || response.result.is_some() == response.error.is_some() {
        return Err(FailureData::invalid_response());
    }
    if let Some(error) = response.error {
        let data = error
            .data
            .ok_or_else(FailureData::invalid_response)
            .and_then(|value| {
                serde_json::from_value::<PluginErrorData>(value)
                    .map_err(|_| FailureData::invalid_response())
            })?;
        data.validate()
            .map_err(|_| FailureData::invalid_response())?;
        return Err(FailureData {
            code: data.code,
            summary: safe_failure_summary(data.code).to_string(),
        });
    }
    response.result.ok_or_else(FailureData::invalid_response)
}

const fn safe_failure_summary(code: PluginErrorCode) -> &'static str {
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
        PluginErrorCode::CredentialNotFound
        | PluginErrorCode::CredentialExpired
        | PluginErrorCode::CredentialScopeNotAllowed
        | PluginErrorCode::LoginFlowNotFound
        | PluginErrorCode::LoginFlowExpired
        | PluginErrorCode::LoginPending
        | PluginErrorCode::LoginDenied => "Content plugin response was invalid",
        PluginErrorCode::RateLimited => "Content source is rate limited",
        PluginErrorCode::PlatformResponseChanged => "Content source response changed",
        PluginErrorCode::PluginInternalError => "Content plugin failed",
    }
}

fn invoke_failure(error: ContentInvokeError) -> FailureData {
    match error {
        ContentInvokeError::Unavailable => FailureData {
            code: PluginErrorCode::PluginUnavailable,
            summary: "Plugin is unavailable".to_string(),
        },
        ContentInvokeError::Timeout => FailureData {
            code: PluginErrorCode::PluginTimeout,
            summary: "Plugin call timed out".to_string(),
        },
        ContentInvokeError::InvalidResponse => FailureData::invalid_response(),
        ContentInvokeError::Internal => FailureData {
            code: PluginErrorCode::PluginInternalError,
            summary: "Plugin call failed".to_string(),
        },
    }
}

struct FailureData {
    code: PluginErrorCode,
    summary: String,
}

impl FailureData {
    fn invalid_response() -> Self {
        Self {
            code: PluginErrorCode::PluginResponseInvalid,
            summary: "Plugin response was invalid".to_string(),
        }
    }

    fn with_source(self, source: ContentSource) -> ContentFailure {
        ContentFailure {
            source,
            code: self.code,
            summary: self.summary,
        }
    }
}
