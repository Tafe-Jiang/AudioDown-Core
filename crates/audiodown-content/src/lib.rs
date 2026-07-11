mod cursor;
mod dedup;
mod router;

pub use cursor::{
    decode_cursor, encode_cursor, ContentCursorBinding, ContentCursorError, ContentCursorOperation,
    SourceCursor, MAX_CURSOR_DECODED_BYTES, MAX_CURSOR_ENCODED_BYTES, MAX_CURSOR_SOURCES,
    MAX_SOURCE_CURSOR_BYTES,
};
pub use dedup::{deduplicate_items, deduplicate_sections};
pub use router::{
    select_candidates, AggregatedDiscoverResult, AggregatedSearchResult, ContentAggregationService,
    ContentCandidate, ContentFailure, ContentFilters, ContentInvokeError, ContentPluginInvoker,
    ContentRouteKind, ContentRoutingStore, ContentServiceError, ContentSource,
    DiscoverAggregationInput, SearchAggregationInput, SourcedContentItem, SourcedDiscoverSection,
};
