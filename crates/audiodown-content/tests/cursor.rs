use audiodown_content::{
    decode_cursor, encode_cursor, ContentCursorBinding, ContentCursorError, ContentCursorOperation,
    ContentFilters, SourceCursor, MAX_CURSOR_DECODED_BYTES, MAX_CURSOR_ENCODED_BYTES,
    MAX_CURSOR_SOURCES, MAX_SOURCE_CURSOR_BYTES,
};
use audiodown_domain::plugin::PluginId;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

#[test]
fn enforces_the_locked_sixteen_kibibyte_decoded_limit() {
    assert_eq!(MAX_CURSOR_DECODED_BYTES, 16 * 1024);
}

#[test]
fn round_trips_url_safe_versioned_source_cursors() {
    let binding = search_binding("needle", Some("virtual"), None);
    let sources = vec![
        source_cursor("catalog", "com.audiodown.catalog.primary", "page:/?=2"),
        source_cursor("virtual", "com.audiodown.virtual.backup", "opaque+/=cursor"),
    ];

    let encoded = encode_cursor(&binding, &sources).unwrap().unwrap();

    assert!(!encoded.contains('='));
    assert!(encoded
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')));
    assert_eq!(decode_cursor(&encoded, &binding).unwrap(), sources);
}

#[test]
fn binds_cursor_to_operation_query_and_filters() {
    let binding = search_binding("needle", Some("virtual"), None);
    let encoded = encode_cursor(
        &binding,
        &[source_cursor(
            "virtual",
            "com.audiodown.virtual.primary",
            "next",
        )],
    )
    .unwrap()
    .unwrap();

    for changed in [
        ContentCursorBinding {
            operation: ContentCursorOperation::Discover,
            query: None,
            filters: binding.filters.clone(),
        },
        search_binding("changed", Some("virtual"), None),
        search_binding("needle", Some("catalog"), None),
        search_binding(
            "needle",
            Some("virtual"),
            Some("com.audiodown.virtual.primary"),
        ),
    ] {
        assert_eq!(
            decode_cursor(&encoded, &changed).unwrap_err(),
            ContentCursorError::BindingMismatch
        );
    }
}

#[test]
fn preserves_selected_plugin_and_per_source_opaque_cursor() {
    let binding = search_binding("needle", None, None);
    let sources = vec![
        source_cursor("alpha", "com.audiodown.alpha.backup", "alpha-next"),
        source_cursor("beta", "com.audiodown.beta.primary", "beta-next"),
    ];

    let encoded = encode_cursor(&binding, &sources).unwrap().unwrap();
    let decoded = decode_cursor(&encoded, &binding).unwrap();

    assert_eq!(decoded[0].plugin_id.as_str(), "com.audiodown.alpha.backup");
    assert_eq!(decoded[0].cursor, "alpha-next");
    assert_eq!(decoded[1].plugin_id.as_str(), "com.audiodown.beta.primary");
    assert_eq!(decoded[1].cursor, "beta-next");
}

#[test]
fn rejects_source_count_and_size_limit_violations() {
    let binding = search_binding("needle", None, None);
    let too_many = (0..=MAX_CURSOR_SOURCES)
        .map(|index| {
            source_cursor(
                &format!("platform-{index}"),
                &format!("com.audiodown.virtual.plugin-{index}"),
                "next",
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        encode_cursor(&binding, &too_many).unwrap_err(),
        ContentCursorError::TooManySources
    );

    let oversized_source = source_cursor(
        "virtual",
        "com.audiodown.virtual.primary",
        &"x".repeat(MAX_SOURCE_CURSOR_BYTES + 1),
    );
    assert_eq!(
        encode_cursor(&binding, &[oversized_source]).unwrap_err(),
        ContentCursorError::SourceCursorTooLarge
    );

    assert_eq!(
        decode_cursor(&"A".repeat(MAX_CURSOR_ENCODED_BYTES + 1), &binding).unwrap_err(),
        ContentCursorError::EncodedTooLarge
    );

    let oversized_decoded = URL_SAFE_NO_PAD.encode(vec![b'x'; MAX_CURSOR_DECODED_BYTES + 1]);
    assert!(oversized_decoded.len() <= MAX_CURSOR_ENCODED_BYTES);
    assert_eq!(
        decode_cursor(&oversized_decoded, &binding).unwrap_err(),
        ContentCursorError::DecodedTooLarge
    );
}

#[test]
fn rejects_malformed_unsupported_and_tampered_shapes() {
    let binding = search_binding("needle", None, None);
    assert_eq!(
        decode_cursor("%%%", &binding).unwrap_err(),
        ContentCursorError::Malformed
    );

    let unsupported =
        URL_SAFE_NO_PAD.encode(br#"{"version":2,"fingerprint":"ignored","sources":[]}"#);
    assert_eq!(
        decode_cursor(&unsupported, &binding).unwrap_err(),
        ContentCursorError::UnsupportedVersion
    );

    let missing_sources = URL_SAFE_NO_PAD.encode(br#"{"version":1,"fingerprint":"ignored"}"#);
    assert_eq!(
        decode_cursor(&missing_sources, &binding).unwrap_err(),
        ContentCursorError::Malformed
    );
}

#[test]
fn no_source_cursor_marks_end_of_pagination() {
    assert_eq!(
        encode_cursor(&search_binding("needle", None, None), &[]).unwrap(),
        None
    );
}

fn search_binding(
    query: &str,
    platform_id: Option<&str>,
    plugin_id: Option<&str>,
) -> ContentCursorBinding {
    ContentCursorBinding {
        operation: ContentCursorOperation::Search,
        query: Some(query.to_string()),
        filters: ContentFilters {
            platform_id: platform_id.map(str::to_string),
            plugin_id: plugin_id.map(|value| PluginId::parse(value).unwrap()),
        },
    }
}

fn source_cursor(platform_id: &str, plugin_id: &str, cursor: &str) -> SourceCursor {
    SourceCursor {
        platform_id: platform_id.to_string(),
        plugin_id: PluginId::parse(plugin_id).unwrap(),
        cursor: cursor.to_string(),
    }
}
