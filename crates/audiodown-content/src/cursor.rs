use audiodown_domain::plugin::PluginId;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ContentFilters;

const CURSOR_VERSION: u8 = 1;

pub const MAX_CURSOR_SOURCES: usize = 32;
pub const MAX_SOURCE_CURSOR_BYTES: usize = 4 * 1024;
pub const MAX_CURSOR_DECODED_BYTES: usize = 16 * 1024;
pub const MAX_CURSOR_ENCODED_BYTES: usize = 24 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentCursorOperation {
    Search,
    Discover,
}

#[derive(Debug, Clone)]
pub struct ContentCursorBinding {
    pub operation: ContentCursorOperation,
    pub query: Option<String>,
    pub filters: ContentFilters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceCursor {
    pub platform_id: String,
    pub plugin_id: PluginId,
    pub cursor: String,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ContentCursorError {
    #[error("cursor encoding exceeds the allowed size")]
    EncodedTooLarge,
    #[error("decoded cursor exceeds the allowed size")]
    DecodedTooLarge,
    #[error("cursor contains too many sources")]
    TooManySources,
    #[error("plugin cursor exceeds the allowed size")]
    SourceCursorTooLarge,
    #[error("cursor is malformed")]
    Malformed,
    #[error("cursor version is unsupported")]
    UnsupportedVersion,
    #[error("cursor does not match the request")]
    BindingMismatch,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorEnvelope {
    version: u8,
    fingerprint: String,
    sources: Vec<SourceCursor>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BindingFingerprint<'a> {
    operation: ContentCursorOperation,
    query: &'a Option<String>,
    platform_id: &'a Option<String>,
    plugin_id: Option<&'a str>,
}

pub fn encode_cursor(
    binding: &ContentCursorBinding,
    sources: &[SourceCursor],
) -> Result<Option<String>, ContentCursorError> {
    if sources.is_empty() {
        return Ok(None);
    }
    validate_sources(sources)?;
    let envelope = CursorEnvelope {
        version: CURSOR_VERSION,
        fingerprint: request_fingerprint(binding)?,
        sources: sources.to_vec(),
    };
    let decoded = serde_json::to_vec(&envelope).map_err(|_| ContentCursorError::Malformed)?;
    if decoded.len() > MAX_CURSOR_DECODED_BYTES {
        return Err(ContentCursorError::DecodedTooLarge);
    }
    let encoded = URL_SAFE_NO_PAD.encode(decoded);
    if encoded.len() > MAX_CURSOR_ENCODED_BYTES {
        return Err(ContentCursorError::EncodedTooLarge);
    }
    Ok(Some(encoded))
}

pub fn decode_cursor(
    encoded: &str,
    binding: &ContentCursorBinding,
) -> Result<Vec<SourceCursor>, ContentCursorError> {
    if encoded.len() > MAX_CURSOR_ENCODED_BYTES {
        return Err(ContentCursorError::EncodedTooLarge);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| ContentCursorError::Malformed)?;
    if decoded.len() > MAX_CURSOR_DECODED_BYTES {
        return Err(ContentCursorError::DecodedTooLarge);
    }
    let envelope = serde_json::from_slice::<CursorEnvelope>(&decoded)
        .map_err(|_| ContentCursorError::Malformed)?;
    if envelope.version != CURSOR_VERSION {
        return Err(ContentCursorError::UnsupportedVersion);
    }
    if envelope.fingerprint != request_fingerprint(binding)? {
        return Err(ContentCursorError::BindingMismatch);
    }
    validate_sources(&envelope.sources)?;
    Ok(envelope.sources)
}

fn request_fingerprint(binding: &ContentCursorBinding) -> Result<String, ContentCursorError> {
    let canonical = BindingFingerprint {
        operation: binding.operation,
        query: &binding.query,
        platform_id: &binding.filters.platform_id,
        plugin_id: binding.filters.plugin_id.as_ref().map(PluginId::as_str),
    };
    let bytes = serde_json::to_vec(&canonical).map_err(|_| ContentCursorError::Malformed)?;
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)))
}

fn validate_sources(sources: &[SourceCursor]) -> Result<(), ContentCursorError> {
    if sources.len() > MAX_CURSOR_SOURCES {
        return Err(ContentCursorError::TooManySources);
    }
    let mut previous_platform = None;
    for source in sources {
        if source.cursor.is_empty() {
            return Err(ContentCursorError::Malformed);
        }
        if source.cursor.len() > MAX_SOURCE_CURSOR_BYTES {
            return Err(ContentCursorError::SourceCursorTooLarge);
        }
        if !valid_platform_id(&source.platform_id) {
            return Err(ContentCursorError::Malformed);
        }
        if previous_platform.is_some_and(|previous| previous >= source.platform_id.as_str()) {
            return Err(ContentCursorError::Malformed);
        }
        previous_platform = Some(source.platform_id.as_str());
    }
    Ok(())
}

fn valid_platform_id(platform_id: &str) -> bool {
    !platform_id.is_empty()
        && platform_id.len() <= 128
        && platform_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}
