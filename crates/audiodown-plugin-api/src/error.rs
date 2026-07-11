use serde::{Deserialize, Serialize};

use crate::content::ContentContractError;

const MAX_ERROR_SUMMARY_BYTES: usize = 512;
const MAX_RETRY_AFTER_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PluginErrorCode {
    InvalidRequest,
    PluginNotFound,
    PluginDisabled,
    PluginCapabilityMissing,
    PluginUnavailable,
    PluginTimeout,
    PluginResponseInvalid,
    ResourceNotFound,
    ResourceAccessDenied,
    ResourceTemporarilyUnavailable,
    RateLimited,
    PlatformResponseChanged,
    PluginInternalError,
}

impl PluginErrorCode {
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::PluginUnavailable
                | Self::PluginTimeout
                | Self::ResourceTemporarilyUnavailable
                | Self::RateLimited
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginErrorData {
    pub code: PluginErrorCode,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

impl PluginErrorData {
    pub fn validate(&self) -> Result<(), ContentContractError> {
        if self.summary.trim().is_empty() || self.summary.len() > MAX_ERROR_SUMMARY_BYTES {
            return Err(ContentContractError::InvalidText("errorSummary"));
        }
        if self
            .retry_after_seconds
            .is_some_and(|seconds| seconds > MAX_RETRY_AFTER_SECONDS)
        {
            return Err(ContentContractError::InvalidText("retryAfterSeconds"));
        }
        Ok(())
    }
}
