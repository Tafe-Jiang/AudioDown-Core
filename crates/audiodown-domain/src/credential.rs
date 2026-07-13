use crate::plugin::PluginId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_CREDENTIAL_SCOPE_BYTES: usize = 128;
pub const MAX_CREDENTIAL_SCOPE_SEGMENTS: usize = 8;
const MAX_CREDENTIAL_SCOPE_SEGMENT_BYTES: usize = 32;
const MIN_CREDENTIAL_SCOPE_SEGMENTS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CredentialScope(String);

impl CredentialScope {
    pub fn parse(input: impl Into<String>) -> Result<Self, CredentialScopeError> {
        let input = input.into();
        if input.is_empty() {
            return Err(CredentialScopeError::Empty);
        }
        if input.len() > MAX_CREDENTIAL_SCOPE_BYTES {
            return Err(CredentialScopeError::TooLong);
        }
        if !input.is_ascii() {
            return Err(CredentialScopeError::InvalidFormat);
        }

        let segments = input.split('.').collect::<Vec<_>>();
        if !(MIN_CREDENTIAL_SCOPE_SEGMENTS..=MAX_CREDENTIAL_SCOPE_SEGMENTS)
            .contains(&segments.len())
        {
            return Err(CredentialScopeError::InvalidSegmentCount);
        }
        if segments.iter().any(|segment| !valid_scope_segment(segment)) {
            return Err(CredentialScopeError::InvalidFormat);
        }

        Ok(Self(input))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CredentialScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for CredentialScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn valid_scope_segment(segment: &str) -> bool {
    if segment.is_empty() || segment.len() > MAX_CREDENTIAL_SCOPE_SEGMENT_BYTES {
        return false;
    }

    let mut bytes = segment.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CredentialScopeError {
    #[error("credential scope cannot be empty")]
    Empty,
    #[error("credential scope cannot exceed 128 bytes")]
    TooLong,
    #[error("credential scope must contain between 2 and 8 segments")]
    InvalidSegmentCount,
    #[error(
        "credential scope segments must start with a lowercase ASCII letter and contain only lowercase ASCII letters or digits"
    )]
    InvalidFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CredentialId(Uuid);

impl CredentialId {
    pub fn parse(input: impl AsRef<str>) -> Result<Self, CredentialIdError> {
        let value =
            Uuid::parse_str(input.as_ref()).map_err(|_| CredentialIdError::InvalidFormat)?;
        Self::from_uuid(value)
    }

    pub fn from_uuid(value: Uuid) -> Result<Self, CredentialIdError> {
        if value.is_nil() {
            return Err(CredentialIdError::Nil);
        }
        Ok(Self(value))
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl<'de> Deserialize<'de> for CredentialId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Uuid::deserialize(deserializer)?;
        Self::from_uuid(value).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for CredentialId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CredentialIdError {
    #[error("credential ID must be a UUID")]
    InvalidFormat,
    #[error("credential ID cannot be nil")]
    Nil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialKind {
    Cookie,
    Token,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialStatus {
    Active,
    Expired,
    Revoked,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "pluginId", rename_all = "snake_case")]
pub enum CredentialOwnership {
    Plugin(PluginId),
    Retained,
}

impl CredentialOwnership {
    pub fn source_plugin_id(&self) -> Option<&PluginId> {
        match self {
            Self::Plugin(plugin_id) => Some(plugin_id),
            Self::Retained => None,
        }
    }

    pub fn is_retained(&self) -> bool {
        matches!(self, Self::Retained)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPublicMetadata {
    pub id: CredentialId,
    pub kind: CredentialKind,
    pub platform_id: String,
    pub scope: CredentialScope,
    pub ownership: CredentialOwnership,
    pub status: CredentialStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
