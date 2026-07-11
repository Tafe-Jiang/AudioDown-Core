use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use audiodown_content::{
    encode_cursor, ContentAggregationService, ContentCandidate, ContentCursorBinding,
    ContentCursorOperation, ContentFailure, ContentFilters, ContentInvokeError,
    ContentPluginInvoker, ContentRouteKind, ContentRoutingStore, DiscoverAggregationInput,
    SearchAggregationInput, SourceCursor,
};
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::{
    content::{
        ContentItem, ContentMethod, ContentResourceType, DiscoverLayout, DiscoverResult,
        DiscoverSection, SearchResult,
    },
    error::{PluginErrorCode, PluginErrorData},
    rpc::{JsonRpcError, JsonRpcResponse},
};

#[tokio::test]
async fn falls_back_on_retryable_first_page_failure_and_keeps_platform_order() {
    let store = Arc::new(FakeStore::new(vec![
        candidate("virtual", "primary", 100, true),
        candidate("virtual", "backup", 10, false),
        candidate("catalog", "primary", 10, false),
    ]));
    let invoker = Arc::new(FakeInvoker::default());
    invoker.queue(
        "com.audiodown.virtual.primary",
        rpc_error(PluginErrorCode::RateLimited),
    );
    invoker.queue(
        "com.audiodown.virtual.backup",
        search_success("virtual-backup"),
    );
    invoker.queue(
        "com.audiodown.catalog.primary",
        search_success("catalog-primary"),
    );
    let service = ContentAggregationService::new(store, invoker.clone());

    let result = service
        .search(SearchAggregationInput {
            request_id: "request-1".to_string(),
            query: "virtual".to_string(),
            limit: 20,
            filters: ContentFilters::default(),
            cursor: None,
        })
        .await
        .unwrap();

    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.item.resource_id.as_str())
            .collect::<Vec<_>>(),
        ["catalog-primary", "virtual-backup"]
    );
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].code, PluginErrorCode::RateLimited);
    assert_eq!(
        invoker.calls(),
        [
            "com.audiodown.catalog.primary:content.search",
            "com.audiodown.virtual.backup:content.search",
            "com.audiodown.virtual.primary:content.search",
        ]
    );
}

#[tokio::test]
async fn explicit_plugin_and_later_pages_never_fall_back() {
    for explicit in [true, false] {
        let primary = candidate("virtual", "primary", 100, true);
        let backup = candidate("virtual", "backup", 10, false);
        let store = Arc::new(FakeStore::new(vec![primary.clone(), backup]));
        let invoker = Arc::new(FakeInvoker::default());
        invoker.queue(
            primary.plugin_id.as_str(),
            Err(ContentInvokeError::Unavailable),
        );
        let service = ContentAggregationService::new(store, invoker.clone());
        let filters = ContentFilters {
            platform_id: None,
            plugin_id: explicit.then_some(primary.plugin_id.clone()),
        };
        let cursor = (!explicit).then(|| {
            encode_cursor(
                &ContentCursorBinding {
                    operation: ContentCursorOperation::Search,
                    query: Some("virtual".to_string()),
                    filters: filters.clone(),
                },
                &[SourceCursor {
                    platform_id: primary.platform_id.clone(),
                    plugin_id: primary.plugin_id.clone(),
                    cursor: "plugin-page-2".to_string(),
                }],
            )
            .unwrap()
            .unwrap()
        });

        let result = service
            .search(SearchAggregationInput {
                request_id: "request-no-fallback".to_string(),
                query: "virtual".to_string(),
                limit: 20,
                filters,
                cursor,
            })
            .await
            .unwrap();
        assert!(result.items.is_empty());
        assert_eq!(result.failures.len(), 1);
        assert_eq!(invoker.calls().len(), 1);
    }
}

#[tokio::test]
async fn non_retryable_failure_does_not_fall_back() {
    let primary = candidate("virtual", "primary", 100, true);
    let store = Arc::new(FakeStore::new(vec![
        primary.clone(),
        candidate("virtual", "backup", 10, false),
    ]));
    let invoker = Arc::new(FakeInvoker::default());
    invoker.queue(
        primary.plugin_id.as_str(),
        rpc_error(PluginErrorCode::ResourceAccessDenied),
    );
    let service = ContentAggregationService::new(store, invoker.clone());

    let result = service.search(search_input()).await.unwrap();
    assert!(result.items.is_empty());
    assert_eq!(result.failures.len(), 1);
    assert_eq!(invoker.calls().len(), 1);
}

#[tokio::test]
async fn one_platform_failure_does_not_discard_other_platform_results() {
    let store = Arc::new(FakeStore::new(vec![
        candidate("virtual", "primary", 10, false),
        candidate("catalog", "primary", 10, false),
    ]));
    let invoker = Arc::new(FakeInvoker::default());
    invoker.queue(
        "com.audiodown.virtual.primary",
        Err(ContentInvokeError::Timeout),
    );
    invoker.queue(
        "com.audiodown.catalog.primary",
        search_success("catalog-result"),
    );
    let service = ContentAggregationService::new(store, invoker);

    let result = service.search(search_input()).await.unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].item.resource_id, "catalog-result");
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].code, PluginErrorCode::PluginTimeout);
}

#[tokio::test]
async fn aggregates_discover_sections_with_trusted_sources() {
    let store = Arc::new(FakeStore::new(vec![candidate(
        "virtual", "primary", 10, false,
    )]));
    let invoker = Arc::new(FakeInvoker::default());
    invoker.queue(
        "com.audiodown.virtual.primary",
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: "plugin-request".to_string(),
            result: Some(
                serde_json::to_value(DiscoverResult {
                    sections: vec![DiscoverSection {
                        id: "featured".to_string(),
                        title: "Featured".to_string(),
                        layout: DiscoverLayout::AlbumGrid,
                        items: vec![item("album-1")],
                    }],
                    next_cursor: None,
                })
                .unwrap(),
            ),
            error: None,
        }),
    );
    let service = ContentAggregationService::new(store, invoker);

    let result = service
        .discover(DiscoverAggregationInput {
            request_id: "discover-request".to_string(),
            limit: 20,
            filters: ContentFilters::default(),
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(result.sections.len(), 1);
    assert_eq!(result.sections[0].source.platform_id, "virtual");
    assert_eq!(
        result.sections[0].source.plugin_id.as_str(),
        "com.audiodown.virtual.primary"
    );
}

fn search_input() -> SearchAggregationInput {
    SearchAggregationInput {
        request_id: "request-search".to_string(),
        query: "virtual".to_string(),
        limit: 20,
        filters: ContentFilters::default(),
        cursor: None,
    }
}

fn candidate(platform_id: &str, suffix: &str, priority: i64, is_default: bool) -> ContentCandidate {
    ContentCandidate {
        plugin_id: PluginId::parse(format!("com.audiodown.{platform_id}.{suffix}")).unwrap(),
        plugin_name: suffix.to_string(),
        plugin_version: "1.0.0".to_string(),
        platform_id: platform_id.to_string(),
        priority,
        is_default,
        search_enabled: true,
        discover_enabled: true,
        capabilities: vec![
            ContentMethod::Search,
            ContentMethod::Discover,
            ContentMethod::Categories,
        ],
    }
}

fn item(resource_id: &str) -> ContentItem {
    ContentItem {
        resource_type: ContentResourceType::Album,
        resource_id: resource_id.to_string(),
        canonical_id: None,
        title: resource_id.to_string(),
        subtitle: None,
        description: None,
    }
}

fn search_success(resource_id: &str) -> Result<JsonRpcResponse, ContentInvokeError> {
    Ok(JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: "plugin-request".to_string(),
        result: Some(
            serde_json::to_value(SearchResult {
                items: vec![item(resource_id)],
                next_cursor: None,
            })
            .unwrap(),
        ),
        error: None,
    })
}

fn rpc_error(code: PluginErrorCode) -> Result<JsonRpcResponse, ContentInvokeError> {
    Ok(JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: "plugin-request".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code: -32000,
            message: "Plugin call failed".to_string(),
            data: Some(
                serde_json::to_value(PluginErrorData {
                    code,
                    summary: "Plugin call failed".to_string(),
                    retry_after_seconds: None,
                })
                .unwrap(),
            ),
        }),
    })
}

struct FakeStore {
    candidates: Vec<ContentCandidate>,
}

impl FakeStore {
    fn new(candidates: Vec<ContentCandidate>) -> Self {
        Self { candidates }
    }
}

#[async_trait]
impl ContentRoutingStore for FakeStore {
    async fn list_candidates(
        &self,
        _route: ContentRouteKind,
    ) -> Result<Vec<ContentCandidate>, audiodown_content::ContentServiceError> {
        Ok(self.candidates.clone())
    }
}

#[derive(Default)]
struct FakeInvoker {
    responses: Mutex<HashMap<String, VecDeque<Result<JsonRpcResponse, ContentInvokeError>>>>,
    calls: Mutex<Vec<String>>,
}

impl FakeInvoker {
    fn queue(&self, plugin_id: &str, response: Result<JsonRpcResponse, ContentInvokeError>) {
        self.responses
            .lock()
            .unwrap()
            .entry(plugin_id.to_string())
            .or_default()
            .push_back(response);
    }

    fn calls(&self) -> Vec<String> {
        let mut calls = self.calls.lock().unwrap().clone();
        calls.sort();
        calls
    }
}

#[async_trait]
impl ContentPluginInvoker for FakeInvoker {
    async fn invoke(
        &self,
        _request_id: &str,
        candidate: &ContentCandidate,
        method: ContentMethod,
        _params: serde_json::Value,
    ) -> Result<JsonRpcResponse, ContentInvokeError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:{}", candidate.plugin_id, method.capability()));
        self.responses
            .lock()
            .unwrap()
            .get_mut(candidate.plugin_id.as_str())
            .and_then(VecDeque::pop_front)
            .unwrap_or(Err(ContentInvokeError::Internal))
    }
}

fn _assert_failure_is_public(_failure: &ContentFailure) {}
