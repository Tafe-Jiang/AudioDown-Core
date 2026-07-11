use audiodown_content::{select_candidates, ContentCandidate, ContentFilters, ContentRouteKind};
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::content::ContentMethod;

#[test]
fn selects_default_then_priority_and_plugin_id_per_platform() {
    let selected = select_candidates(
        vec![
            candidate("virtual", "zeta", 10, false),
            candidate("virtual", "default", 100, true),
            candidate("virtual", "alpha", 10, false),
            candidate("catalog", "only", 50, false),
        ],
        ContentMethod::Search,
        ContentRouteKind::Search,
        &ContentFilters::default(),
    );

    assert_eq!(
        selected
            .iter()
            .map(|candidate| candidate.plugin_id.as_str())
            .collect::<Vec<_>>(),
        [
            "com.audiodown.catalog.only",
            "com.audiodown.virtual.default",
            "com.audiodown.virtual.alpha",
            "com.audiodown.virtual.zeta",
        ]
    );
}

#[test]
fn applies_capability_participation_platform_and_plugin_filters() {
    let mut search_only = candidate("virtual", "search", 10, false);
    search_only.discover_enabled = false;
    let mut discover_only = candidate("virtual", "discover", 20, false);
    discover_only.search_enabled = false;
    let mut missing_capability = candidate("catalog", "missing", 10, false);
    missing_capability.capabilities = vec![ContentMethod::Discover];

    let search = select_candidates(
        vec![
            search_only.clone(),
            discover_only.clone(),
            missing_capability,
        ],
        ContentMethod::Search,
        ContentRouteKind::Search,
        &ContentFilters {
            platform_id: Some("virtual".to_string()),
            plugin_id: None,
        },
    );
    assert_eq!(search.len(), 1);
    assert_eq!(search[0].plugin_id, search_only.plugin_id);

    let discover = select_candidates(
        vec![search_only, discover_only.clone()],
        ContentMethod::Discover,
        ContentRouteKind::Discover,
        &ContentFilters {
            platform_id: None,
            plugin_id: Some(discover_only.plugin_id.clone()),
        },
    );
    assert_eq!(discover.len(), 1);
    assert_eq!(discover[0].plugin_id, discover_only.plugin_id);
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
