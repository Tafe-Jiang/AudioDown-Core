use audiodown_content::{deduplicate_items, ContentSource, SourcedContentItem};
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::content::{ContentItem, ContentResourceType};

#[test]
fn deduplicates_only_matching_resource_type_and_nonempty_canonical_id() {
    let deduplicated = deduplicate_items(vec![
        sourced(
            ContentResourceType::Album,
            "album-a",
            Some("shared"),
            "first",
        ),
        sourced(
            ContentResourceType::Album,
            "album-b",
            Some("shared"),
            "duplicate",
        ),
        sourced(
            ContentResourceType::Track,
            "track-a",
            Some("shared"),
            "track",
        ),
        sourced(ContentResourceType::Album, "album-c", None, "without-id"),
        sourced(ContentResourceType::Album, "album-d", Some(""), "empty-id"),
        sourced(
            ContentResourceType::Album,
            "album-e",
            Some(""),
            "second-empty-id",
        ),
    ]);

    assert_eq!(
        deduplicated
            .iter()
            .map(|item| item.item.resource_id.as_str())
            .collect::<Vec<_>>(),
        ["album-a", "track-a", "album-c", "album-d", "album-e"]
    );
}

#[test]
fn keeps_first_deterministic_item_without_merging_untrusted_metadata() {
    let deduplicated = deduplicate_items(vec![
        sourced(
            ContentResourceType::Album,
            "trusted-first",
            Some("canonical"),
            "First title",
        ),
        sourced(
            ContentResourceType::Album,
            "later",
            Some("canonical"),
            "Later title",
        ),
    ]);

    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].item.resource_id, "trusted-first");
    assert_eq!(deduplicated[0].item.title, "First title");
    assert_eq!(
        deduplicated[0].source.plugin_id.as_str(),
        "com.audiodown.virtual.trusted-first"
    );
}

fn sourced(
    resource_type: ContentResourceType,
    resource_id: &str,
    canonical_id: Option<&str>,
    title: &str,
) -> SourcedContentItem {
    SourcedContentItem {
        item: ContentItem {
            resource_type,
            resource_id: resource_id.to_string(),
            canonical_id: canonical_id.map(str::to_string),
            title: title.to_string(),
            subtitle: None,
            description: None,
        },
        source: ContentSource {
            plugin_id: PluginId::parse(format!("com.audiodown.virtual.{resource_id}")).unwrap(),
            plugin_name: resource_id.to_string(),
            plugin_version: "1.0.0".to_string(),
            platform_id: "virtual".to_string(),
        },
    }
}
