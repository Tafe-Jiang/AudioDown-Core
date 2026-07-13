use audiodown_plugin_api::{
    content::{
        AlbumDetail, AlbumGetRequest, AlbumGetResult, CategoriesRequest, CategoriesResult,
        CategoryItem, ContentItem, ContentMethod, ContentResourceType, DiscoverLayout,
        DiscoverRequest, DiscoverResult, DiscoverSection, SearchRequest, SearchResult, TrackItem,
        TracksListRequest, TracksListResult, MAX_CURSOR_BYTES, MAX_ITEMS_PER_RESPONSE,
        MAX_OPAQUE_ID_BYTES, MAX_QUERY_BYTES,
    },
    credential::CredentialMethod,
    error::{PluginErrorCode, PluginErrorData},
};
use serde_json::json;

fn content_item(resource_id: &str, canonical_id: Option<&str>) -> ContentItem {
    ContentItem {
        resource_type: ContentResourceType::Album,
        resource_id: resource_id.to_string(),
        canonical_id: canonical_id.map(str::to_string),
        title: "Virtual Album".to_string(),
        subtitle: Some("Virtual Creator".to_string()),
        description: None,
    }
}

#[test]
fn serializes_only_the_five_phase_three_methods() {
    let methods = [
        (ContentMethod::Search, "\"content.search\""),
        (ContentMethod::Discover, "\"content.discover\""),
        (ContentMethod::Categories, "\"content.categories\""),
        (ContentMethod::AlbumGet, "\"content.album.get\""),
        (ContentMethod::TracksList, "\"content.tracks.list\""),
    ];

    for (method, expected) in methods {
        assert_eq!(serde_json::to_string(&method).unwrap(), expected);
        assert_eq!(
            serde_json::from_str::<ContentMethod>(expected).unwrap(),
            method
        );
        assert_eq!(method.capability(), expected.trim_matches('"'));
    }

    assert!(serde_json::from_str::<ContentMethod>("\"content.download.plan\"").is_err());
    assert!(serde_json::from_str::<ContentMethod>("\"credential.status\"").is_err());
    assert!(serde_json::from_str::<CredentialMethod>("\"content.search\"").is_err());
}

#[test]
fn round_trips_strict_search_and_discover_contracts() {
    let search: SearchRequest = serde_json::from_value(json!({
        "query": "virtual",
        "cursor": "opaque-search-cursor",
        "limit": 20
    }))
    .unwrap();
    search.validate().unwrap();
    assert_eq!(search.query, "virtual");

    let result = SearchResult {
        items: vec![content_item("album-1", Some("virtual:album:1"))],
        next_cursor: Some("next-search-cursor".to_string()),
    };
    result.validate().unwrap();
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["items"][0]["resourceType"], "album");
    assert_eq!(value["nextCursor"], "next-search-cursor");

    let discover = DiscoverResult {
        sections: vec![DiscoverSection {
            id: "featured".to_string(),
            title: "Featured".to_string(),
            layout: DiscoverLayout::HeroCarousel,
            items: result.items.clone(),
        }],
        next_cursor: None,
    };
    discover.validate().unwrap();
    let value = serde_json::to_value(&discover).unwrap();
    assert_eq!(value["sections"][0]["layout"], "hero-carousel");

    assert!(serde_json::from_value::<SearchRequest>(json!({
        "query": "virtual",
        "limit": 20,
        "unexpected": true
    }))
    .is_err());
    assert!(serde_json::from_value::<DiscoverRequest>(json!({
        "limit": 20,
        "unexpected": true
    }))
    .is_err());
}

#[test]
fn round_trips_categories_album_and_track_pagination() {
    let categories = CategoriesResult {
        items: vec![CategoryItem {
            resource_id: "category-1".to_string(),
            canonical_id: Some("virtual:category:1".to_string()),
            title: "Virtual Category".to_string(),
            description: None,
        }],
    };
    categories.validate().unwrap();
    serde_json::from_value::<CategoriesRequest>(json!({})).unwrap();

    let album_request: AlbumGetRequest =
        serde_json::from_value(json!({"resourceId": "album-1"})).unwrap();
    album_request.validate().unwrap();
    let album = AlbumGetResult {
        album: AlbumDetail {
            resource_id: "album-1".to_string(),
            canonical_id: Some("virtual:album:1".to_string()),
            title: "Virtual Album".to_string(),
            creator: Some("Virtual Creator".to_string()),
            description: Some("Deterministic fixture album".to_string()),
            track_count: Some(2),
        },
    };
    album.validate().unwrap();

    let tracks_request: TracksListRequest = serde_json::from_value(json!({
        "albumResourceId": "album-1",
        "cursor": "opaque-track-cursor",
        "limit": 50
    }))
    .unwrap();
    tracks_request.validate().unwrap();
    let tracks = TracksListResult {
        items: vec![TrackItem {
            resource_id: "track-1".to_string(),
            canonical_id: Some("virtual:track:1".to_string()),
            title: "Virtual Track".to_string(),
            subtitle: None,
            sequence: Some(1),
            duration_seconds: Some(60),
        }],
        next_cursor: None,
    };
    tracks.validate().unwrap();

    assert_eq!(
        serde_json::to_value(&album).unwrap()["album"]["trackCount"],
        2
    );
    assert_eq!(
        serde_json::to_value(&tracks).unwrap()["items"][0]["durationSeconds"],
        60
    );
}

#[test]
fn rejects_oversized_or_invalid_content_values() {
    let mut request = SearchRequest {
        query: "x".repeat(MAX_QUERY_BYTES + 1),
        cursor: None,
        limit: 20,
    };
    assert!(request.validate().is_err());

    request.query = "virtual".to_string();
    request.cursor = Some("x".repeat(MAX_CURSOR_BYTES + 1));
    assert!(request.validate().is_err());

    let invalid_limit = SearchRequest {
        query: "virtual".to_string(),
        cursor: None,
        limit: 0,
    };
    assert!(invalid_limit.validate().is_err());

    let invalid_resource = AlbumGetRequest {
        resource_id: "x".repeat(MAX_OPAQUE_ID_BYTES + 1),
    };
    assert!(invalid_resource.validate().is_err());

    let too_many_items = SearchResult {
        items: (0..=MAX_ITEMS_PER_RESPONSE)
            .map(|index| content_item(&format!("album-{index}"), None))
            .collect(),
        next_cursor: None,
    };
    assert!(too_many_items.validate().is_err());

    let blank_title = SearchResult {
        items: vec![ContentItem {
            title: "   ".to_string(),
            ..content_item("album-1", None)
        }],
        next_cursor: None,
    };
    assert!(blank_title.validate().is_err());
}

#[test]
fn classifies_standard_errors_without_raw_details() {
    let retryable = [
        PluginErrorCode::PluginUnavailable,
        PluginErrorCode::PluginTimeout,
        PluginErrorCode::ResourceTemporarilyUnavailable,
        PluginErrorCode::RateLimited,
    ];
    for code in retryable {
        assert!(code.is_retryable());
    }
    assert!(!PluginErrorCode::ResourceAccessDenied.is_retryable());
    assert!(!PluginErrorCode::PluginResponseInvalid.is_retryable());

    let error: PluginErrorData = serde_json::from_value(json!({
        "code": "RATE_LIMITED",
        "summary": "The virtual source asked Core to retry later",
        "retryAfterSeconds": 30
    }))
    .unwrap();
    error.validate().unwrap();
    assert_eq!(error.code, PluginErrorCode::RateLimited);
    assert_eq!(error.retry_after_seconds, Some(30));

    assert!(serde_json::from_value::<PluginErrorData>(json!({
        "code": "RATE_LIMITED",
        "summary": "safe",
        "rawStack": "must not cross the contract"
    }))
    .is_err());
}
