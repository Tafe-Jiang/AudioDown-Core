use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_QUERY_BYTES: usize = 512;
pub const MAX_CURSOR_BYTES: usize = 4 * 1024;
pub const MAX_OPAQUE_ID_BYTES: usize = 1024;
pub const MAX_ITEMS_PER_RESPONSE: usize = 200;
pub const MAX_SECTIONS_PER_RESPONSE: usize = 32;

const MAX_TITLE_BYTES: usize = 512;
const MAX_SHORT_TEXT_BYTES: usize = 1024;
const MAX_DESCRIPTION_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentMethod {
    #[serde(rename = "content.search")]
    Search,
    #[serde(rename = "content.discover")]
    Discover,
    #[serde(rename = "content.categories")]
    Categories,
    #[serde(rename = "content.album.get")]
    AlbumGet,
    #[serde(rename = "content.tracks.list")]
    TracksList,
}

impl ContentMethod {
    pub const ALL: [Self; 5] = [
        Self::Search,
        Self::Discover,
        Self::Categories,
        Self::AlbumGet,
        Self::TracksList,
    ];

    pub const fn capability(self) -> &'static str {
        match self {
            Self::Search => "content.search",
            Self::Discover => "content.discover",
            Self::Categories => "content.categories",
            Self::AlbumGet => "content.album.get",
            Self::TracksList => "content.tracks.list",
        }
    }

    pub fn from_capability(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|method| method.capability() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentResourceType {
    Album,
    Track,
    Category,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub limit: u16,
}

impl SearchRequest {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_nonempty(&self.query, MAX_QUERY_BYTES, "query")?;
        validate_limit(self.limit)?;
        validate_optional_opaque(&self.cursor, MAX_CURSOR_BYTES, "cursor")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchResult {
    pub items: Vec<ContentItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl SearchResult {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_item_count(self.items.len())?;
        for item in &self.items {
            item.validate()?;
        }
        validate_optional_opaque(&self.next_cursor, MAX_CURSOR_BYTES, "nextCursor")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub limit: u16,
}

impl DiscoverRequest {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_limit(self.limit)?;
        validate_optional_opaque(&self.cursor, MAX_CURSOR_BYTES, "cursor")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverResult {
    pub sections: Vec<DiscoverSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl DiscoverResult {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        if self.sections.len() > MAX_SECTIONS_PER_RESPONSE {
            return Err(ContentContractError::TooManySections);
        }
        for section in &self.sections {
            section.validate()?;
        }
        validate_optional_opaque(&self.next_cursor, MAX_CURSOR_BYTES, "nextCursor")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoriesRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CategoriesResult {
    pub items: Vec<CategoryItem>,
}

impl CategoriesResult {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_item_count(self.items.len())?;
        for item in &self.items {
            item.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlbumGetRequest {
    pub resource_id: String,
}

impl AlbumGetRequest {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_opaque(&self.resource_id, MAX_OPAQUE_ID_BYTES, "resourceId")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlbumGetResult {
    pub album: AlbumDetail,
}

impl AlbumGetResult {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        self.album.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TracksListRequest {
    pub album_resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub limit: u16,
}

impl TracksListRequest {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_opaque(
            &self.album_resource_id,
            MAX_OPAQUE_ID_BYTES,
            "albumResourceId",
        )?;
        validate_limit(self.limit)?;
        validate_optional_opaque(&self.cursor, MAX_CURSOR_BYTES, "cursor")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TracksListResult {
    pub items: Vec<TrackItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl TracksListResult {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_item_count(self.items.len())?;
        for item in &self.items {
            item.validate()?;
        }
        validate_optional_opaque(&self.next_cursor, MAX_CURSOR_BYTES, "nextCursor")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentItem {
    pub resource_type: ContentResourceType,
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ContentItem {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_opaque(&self.resource_id, MAX_OPAQUE_ID_BYTES, "resourceId")?;
        validate_optional_opaque(&self.canonical_id, MAX_OPAQUE_ID_BYTES, "canonicalId")?;
        validate_nonempty(&self.title, MAX_TITLE_BYTES, "title")?;
        validate_optional_text(&self.subtitle, MAX_SHORT_TEXT_BYTES, "subtitle")?;
        validate_optional_text(&self.description, MAX_DESCRIPTION_BYTES, "description")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverLayout {
    HeroCarousel,
    AlbumGrid,
    HorizontalList,
    RankedList,
    CategoryGrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverSection {
    pub id: String,
    pub title: String,
    pub layout: DiscoverLayout,
    pub items: Vec<ContentItem>,
}

impl DiscoverSection {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_opaque(&self.id, MAX_OPAQUE_ID_BYTES, "sectionId")?;
        validate_nonempty(&self.title, MAX_TITLE_BYTES, "sectionTitle")?;
        validate_item_count(self.items.len())?;
        for item in &self.items {
            item.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CategoryItem {
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl CategoryItem {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_opaque(&self.resource_id, MAX_OPAQUE_ID_BYTES, "resourceId")?;
        validate_optional_opaque(&self.canonical_id, MAX_OPAQUE_ID_BYTES, "canonicalId")?;
        validate_nonempty(&self.title, MAX_TITLE_BYTES, "title")?;
        validate_optional_text(&self.description, MAX_DESCRIPTION_BYTES, "description")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlbumDetail {
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_count: Option<u32>,
}

impl AlbumDetail {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_opaque(&self.resource_id, MAX_OPAQUE_ID_BYTES, "resourceId")?;
        validate_optional_opaque(&self.canonical_id, MAX_OPAQUE_ID_BYTES, "canonicalId")?;
        validate_nonempty(&self.title, MAX_TITLE_BYTES, "title")?;
        validate_optional_text(&self.creator, MAX_SHORT_TEXT_BYTES, "creator")?;
        validate_optional_text(&self.description, MAX_DESCRIPTION_BYTES, "description")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackItem {
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u32>,
}

impl TrackItem {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        validate_opaque(&self.resource_id, MAX_OPAQUE_ID_BYTES, "resourceId")?;
        validate_optional_opaque(&self.canonical_id, MAX_OPAQUE_ID_BYTES, "canonicalId")?;
        validate_nonempty(&self.title, MAX_TITLE_BYTES, "title")?;
        validate_optional_text(&self.subtitle, MAX_SHORT_TEXT_BYTES, "subtitle")
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ContentContractError {
    #[error("{0} must be non-empty and within its byte limit")]
    InvalidText(&'static str),
    #[error("{0} must be a non-empty opaque value within its byte limit")]
    InvalidOpaqueValue(&'static str),
    #[error("limit must be between 1 and 200")]
    InvalidLimit,
    #[error("content response contains too many items")]
    TooManyItems,
    #[error("discover response contains too many sections")]
    TooManySections,
}

fn validate_limit(limit: u16) -> Result<(), ContentContractError> {
    if (1..=MAX_ITEMS_PER_RESPONSE as u16).contains(&limit) {
        Ok(())
    } else {
        Err(ContentContractError::InvalidLimit)
    }
}

fn validate_item_count(count: usize) -> Result<(), ContentContractError> {
    if count <= MAX_ITEMS_PER_RESPONSE {
        Ok(())
    } else {
        Err(ContentContractError::TooManyItems)
    }
}

fn validate_nonempty(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), ContentContractError> {
    if value.trim().is_empty() || value.len() > maximum {
        Err(ContentContractError::InvalidText(field))
    } else {
        Ok(())
    }
}

fn validate_opaque(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), ContentContractError> {
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        Err(ContentContractError::InvalidOpaqueValue(field))
    } else {
        Ok(())
    }
}

fn validate_optional_opaque(
    value: &Option<String>,
    maximum: usize,
    field: &'static str,
) -> Result<(), ContentContractError> {
    match value {
        Some(value) => validate_opaque(value, maximum, field),
        None => Ok(()),
    }
}

fn validate_optional_text(
    value: &Option<String>,
    maximum: usize,
    field: &'static str,
) -> Result<(), ContentContractError> {
    match value {
        Some(value) => validate_nonempty(value, maximum, field),
        None => Ok(()),
    }
}
