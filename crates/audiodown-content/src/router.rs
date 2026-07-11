use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::{
    content::{
        ContentItem, ContentMethod, DiscoverRequest, DiscoverResult, DiscoverSection,
        SearchRequest, SearchResult,
    },
    error::{PluginErrorCode, PluginErrorData},
    rpc::JsonRpcResponse,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    pub failures: Vec<ContentFailure>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedDiscoverResult {
    pub sections: Vec<SourcedDiscoverSection>,
    pub failures: Vec<ContentFailure>,
}

#[derive(Debug, Clone)]
pub struct SearchAggregationInput {
    pub request_id: String,
    pub query: String,
    pub limit: u16,
    pub filters: ContentFilters,
    pub first_page: bool,
}

#[derive(Debug, Clone)]
pub struct DiscoverAggregationInput {
    pub request_id: String,
    pub limit: u16,
    pub filters: ContentFilters,
    pub first_page: bool,
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
        let candidates = self.store.list_candidates(ContentRouteKind::Search).await?;
        let groups = group_candidates(select_candidates(
            candidates,
            ContentMethod::Search,
            ContentRouteKind::Search,
            &input.filters,
        ));
        let params = serde_json::to_value(SearchRequest {
            query: input.query,
            cursor: None,
            limit: input.limit,
        })
        .map_err(|_| ContentServiceError::RequestSerialization)?;
        let allow_fallback = input.first_page && input.filters.plugin_id.is_none();
        let invoker = Arc::clone(&self.invoker);
        let request_id = input.request_id;
        let mut platform_results = stream::iter(groups.into_iter().enumerate())
            .map(|(index, candidates)| {
                let invoker = Arc::clone(&invoker);
                let request_id = request_id.clone();
                let params = params.clone();
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
        for (_, result) in platform_results {
            aggregate.items.extend(result.items);
            aggregate.failures.extend(result.failures);
        }
        Ok(aggregate)
    }

    pub async fn discover(
        &self,
        input: DiscoverAggregationInput,
    ) -> Result<AggregatedDiscoverResult, ContentServiceError> {
        let candidates = self
            .store
            .list_candidates(ContentRouteKind::Discover)
            .await?;
        let groups = group_candidates(select_candidates(
            candidates,
            ContentMethod::Discover,
            ContentRouteKind::Discover,
            &input.filters,
        ));
        let params = serde_json::to_value(DiscoverRequest {
            cursor: None,
            limit: input.limit,
        })
        .map_err(|_| ContentServiceError::RequestSerialization)?;
        let allow_fallback = input.first_page && input.filters.plugin_id.is_none();
        let invoker = Arc::clone(&self.invoker);
        let request_id = input.request_id;
        let mut platform_results = stream::iter(groups.into_iter().enumerate())
            .map(|(index, candidates)| {
                let invoker = Arc::clone(&invoker);
                let request_id = request_id.clone();
                let params = params.clone();
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
        for (_, result) in platform_results {
            aggregate.sections.extend(result.sections);
            aggregate.failures.extend(result.failures);
        }
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

async fn search_platform<I: ContentPluginInvoker>(
    invoker: &I,
    request_id: &str,
    candidates: Vec<ContentCandidate>,
    params: serde_json::Value,
    allow_fallback: bool,
) -> AggregatedSearchResult {
    let mut aggregate = AggregatedSearchResult::default();
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
                    aggregate.items.extend(result.items.into_iter().map(|item| {
                        SourcedContentItem {
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

async fn discover_platform<I: ContentPluginInvoker>(
    invoker: &I,
    request_id: &str,
    candidates: Vec<ContentCandidate>,
    params: serde_json::Value,
    allow_fallback: bool,
) -> AggregatedDiscoverResult {
    let mut aggregate = AggregatedDiscoverResult::default();
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
                    aggregate
                        .sections
                        .extend(result.sections.into_iter().map(|section| {
                            SourcedDiscoverSection {
                                section,
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
            summary: data.summary,
        });
    }
    response.result.ok_or_else(FailureData::invalid_response)
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
