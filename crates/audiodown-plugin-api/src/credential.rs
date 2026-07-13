use audiodown_domain::credential::{CredentialId, CredentialScope, CredentialStatus};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_PLUGIN_OPAQUE_STATE_BYTES: usize = 4 * 1024;
pub const MAX_QR_PAYLOAD_BYTES: usize = 4 * 1024;

const MAX_COOKIE_JAR_SESSION_ID_BYTES: usize = 256;
const MAX_QR_DISPLAY_CODE_BYTES: usize = 128;
const MAX_ACCOUNT_TEXT_BYTES: usize = 256;
const MAX_QR_EXPIRES_SECONDS: u32 = 60 * 60;
const MAX_POLL_INTERVAL_SECONDS: u16 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialMethod {
    #[serde(rename = "credential.qr.start")]
    QrStart,
    #[serde(rename = "credential.qr.poll")]
    QrPoll,
    #[serde(rename = "credential.import")]
    Import,
    #[serde(rename = "credential.status")]
    Status,
    #[serde(rename = "credential.refresh")]
    Refresh,
    #[serde(rename = "credential.logout")]
    Logout,
}

impl CredentialMethod {
    pub const ALL: [Self; 6] = [
        Self::QrStart,
        Self::QrPoll,
        Self::Import,
        Self::Status,
        Self::Refresh,
        Self::Logout,
    ];

    pub const fn capability(self) -> &'static str {
        match self {
            Self::QrStart => "credential.qr.start",
            Self::QrPoll => "credential.qr.poll",
            Self::Import => "credential.import",
            Self::Status => "credential.status",
            Self::Refresh => "credential.refresh",
            Self::Logout => "credential.logout",
        }
    }

    pub fn from_capability(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|method| method.capability() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PluginOpaqueState(String);

impl PluginOpaqueState {
    pub fn parse(value: impl Into<String>) -> Result<Self, CredentialContractError> {
        let value = value.into();
        validate_opaque(&value, MAX_PLUGIN_OPAQUE_STATE_BYTES, "pluginOpaqueState")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PluginOpaqueState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialQrStartRequest {
    pub scope: CredentialScope,
    pub cookie_jar_session_id: String,
}

impl CredentialQrStartRequest {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        validate_cookie_jar_session_id(&self.cookie_jar_session_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialQrStartResult {
    pub presentation: QrPresentation,
}

impl CredentialQrStartResult {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        self.presentation.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QrPresentation {
    pub payload: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_code: Option<String>,
    pub expires_in_seconds: u32,
    pub poll_interval_seconds: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_state: Option<PluginOpaqueState>,
}

impl QrPresentation {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        validate_opaque(&self.payload, MAX_QR_PAYLOAD_BYTES, "qrPayload")?;
        validate_optional_text(
            &self.display_code,
            MAX_QR_DISPLAY_CODE_BYTES,
            "qrDisplayCode",
        )?;
        if self.expires_in_seconds == 0 || self.expires_in_seconds > MAX_QR_EXPIRES_SECONDS {
            return Err(CredentialContractError::InvalidDuration("expiresInSeconds"));
        }
        validate_poll_interval(self.poll_interval_seconds)?;
        if u32::from(self.poll_interval_seconds) > self.expires_in_seconds {
            return Err(CredentialContractError::InvalidDuration(
                "pollIntervalSeconds",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialQrPollRequest {
    pub scope: CredentialScope,
    pub cookie_jar_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_state: Option<PluginOpaqueState>,
}

impl CredentialQrPollRequest {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        validate_cookie_jar_session_id(&self.cookie_jar_session_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QrPollStatus {
    Pending,
    Scanned,
    Confirmed,
    Expired,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialQrPollResult {
    pub status: QrPollStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_poll_seconds: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_state: Option<PluginOpaqueState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion: Option<CredentialPromotionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<CredentialAccountStatus>,
}

impl CredentialQrPollResult {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        match self.status {
            QrPollStatus::Pending | QrPollStatus::Scanned => {
                validate_poll_interval(
                    self.next_poll_seconds
                        .ok_or(CredentialContractError::InvalidPollState)?,
                )?;
                if self.promotion.is_some() || self.account.is_some() {
                    return Err(CredentialContractError::InvalidPollState);
                }
            }
            QrPollStatus::Confirmed => {
                if self.next_poll_seconds.is_some()
                    || self.promotion.is_none()
                    || self.account.is_none()
                    || self
                        .account
                        .as_ref()
                        .is_some_and(|account| account.status != CredentialStatus::Active)
                {
                    return Err(CredentialContractError::InvalidPollState);
                }
            }
            QrPollStatus::Expired | QrPollStatus::Denied => {
                if self.next_poll_seconds.is_some()
                    || self.promotion.is_some()
                    || self.account.is_some()
                {
                    return Err(CredentialContractError::InvalidPollState);
                }
            }
        }

        if let Some(account) = &self.account {
            account.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialPromotionRequest {
    pub scope: CredentialScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialAccountStatus {
    pub status: CredentialStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl CredentialAccountStatus {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        validate_optional_text(
            &self.account_id_hint,
            MAX_ACCOUNT_TEXT_BYTES,
            "accountIdHint",
        )?;
        validate_optional_text(&self.display_name, MAX_ACCOUNT_TEXT_BYTES, "displayName")
    }
}

macro_rules! credential_request {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name {
            pub credential_id: CredentialId,
            pub scope: CredentialScope,
        }

        impl $name {
            pub fn validate(&self) -> Result<(), CredentialContractError> {
                Ok(())
            }
        }
    };
}

credential_request!(CredentialImportRequest);
credential_request!(CredentialStatusRequest);
credential_request!(CredentialLogoutRequest);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRefreshRequest {
    pub credential_id: CredentialId,
    pub scope: CredentialScope,
    pub cookie_jar_session_id: String,
}

impl CredentialRefreshRequest {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        validate_cookie_jar_session_id(&self.cookie_jar_session_id)
    }
}

macro_rules! account_result {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name {
            pub account: CredentialAccountStatus,
        }

        impl $name {
            pub fn validate(&self) -> Result<(), CredentialContractError> {
                self.account.validate()
            }
        }
    };
}

account_result!(CredentialImportResult);
account_result!(CredentialStatusResult);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRefreshResult {
    pub account: CredentialAccountStatus,
}

impl CredentialRefreshResult {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        self.account.validate()?;
        if self.account.status == CredentialStatus::Active {
            Ok(())
        } else {
            Err(CredentialContractError::InvalidAccountStatus)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialLogoutResult {
    pub status: CredentialStatus,
}

impl CredentialLogoutResult {
    pub fn validate(&self) -> Result<(), CredentialContractError> {
        if self.status == CredentialStatus::Revoked {
            Ok(())
        } else {
            Err(CredentialContractError::InvalidLogoutStatus)
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CredentialContractError {
    #[error("{0} must be a non-empty value within its byte limit")]
    InvalidOpaqueValue(&'static str),
    #[error("{0} must be non-empty safe text within its byte limit")]
    InvalidText(&'static str),
    #[error("{0} is outside its allowed duration")]
    InvalidDuration(&'static str),
    #[error("QR poll fields are inconsistent with the reported status")]
    InvalidPollState,
    #[error("successful credential operation must report an active account")]
    InvalidAccountStatus,
    #[error("credential logout must report revoked status")]
    InvalidLogoutStatus,
}

fn validate_cookie_jar_session_id(value: &str) -> Result<(), CredentialContractError> {
    validate_opaque(value, MAX_COOKIE_JAR_SESSION_ID_BYTES, "cookieJarSessionId")
}

fn validate_poll_interval(value: u16) -> Result<(), CredentialContractError> {
    if (1..=MAX_POLL_INTERVAL_SECONDS).contains(&value) {
        Ok(())
    } else {
        Err(CredentialContractError::InvalidDuration(
            "pollIntervalSeconds",
        ))
    }
}

fn validate_opaque(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), CredentialContractError> {
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        Err(CredentialContractError::InvalidOpaqueValue(field))
    } else {
        Ok(())
    }
}

fn validate_optional_text(
    value: &Option<String>,
    maximum: usize,
    field: &'static str,
) -> Result<(), CredentialContractError> {
    match value {
        Some(value)
            if value.trim().is_empty()
                || value.len() > maximum
                || value.chars().any(char::is_control) =>
        {
            Err(CredentialContractError::InvalidText(field))
        }
        Some(_) | None => Ok(()),
    }
}
