use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PluginId(String);

impl PluginId {
    pub fn parse(input: impl Into<String>) -> Result<Self, PluginIdError> {
        let input = input.into();
        if input.is_empty() {
            return Err(PluginIdError::Empty);
        }
        if input.len() > 128 {
            return Err(PluginIdError::TooLong);
        }

        let pattern = Regex::new(r"^[a-z0-9](?:[a-z0-9._-]*[a-z0-9])?$")
            .expect("plugin ID validation regex must compile");
        if !pattern.is_match(&input) {
            return Err(PluginIdError::InvalidFormat);
        }

        Ok(Self(input))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Installing,
    Installed,
    Starting,
    Healthy,
    Stopped,
    Unhealthy,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    OnDemand,
    Always,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginIdError {
    #[error("plugin ID cannot be empty")]
    Empty,
    #[error("plugin ID cannot exceed 128 characters")]
    TooLong,
    #[error("plugin ID must use lowercase ASCII letters, digits, '.', '_', or '-' and begin and end with a letter or digit")]
    InvalidFormat,
}
