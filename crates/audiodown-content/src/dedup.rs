use std::collections::HashSet;

use audiodown_plugin_api::content::{ContentItem, ContentResourceType};

use crate::{SourcedCategoryItem, SourcedContentItem, SourcedDiscoverSection};

pub fn deduplicate_items(items: Vec<SourcedContentItem>) -> Vec<SourcedContentItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| keep_item(&item.item, &mut seen))
        .collect()
}

pub fn deduplicate_sections(sections: &mut [SourcedDiscoverSection]) {
    let mut seen = HashSet::new();
    for sourced in sections {
        sourced
            .section
            .items
            .retain(|item| keep_item(item, &mut seen));
    }
}

pub fn deduplicate_categories(items: Vec<SourcedCategoryItem>) -> Vec<SourcedCategoryItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| {
            item.item
                .canonical_id
                .as_deref()
                .filter(|canonical_id| !canonical_id.is_empty())
                .is_none_or(|canonical_id| seen.insert(canonical_id.to_string()))
        })
        .collect()
}

fn keep_item(item: &ContentItem, seen: &mut HashSet<(u8, String)>) -> bool {
    let Some(canonical_id) = item
        .canonical_id
        .as_deref()
        .filter(|canonical_id| !canonical_id.is_empty())
    else {
        return true;
    };
    seen.insert((
        resource_type_key(item.resource_type),
        canonical_id.to_string(),
    ))
}

const fn resource_type_key(resource_type: ContentResourceType) -> u8 {
    match resource_type {
        ContentResourceType::Album => 0,
        ContentResourceType::Track => 1,
        ContentResourceType::Category => 2,
    }
}
