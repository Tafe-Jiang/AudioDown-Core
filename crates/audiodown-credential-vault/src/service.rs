use std::{fmt, sync::Arc};

use async_trait::async_trait;
use audiodown_domain::{
    credential::{
        CredentialId, CredentialKind, CredentialOwnership, CredentialScope, CredentialStatus,
    },
    plugin::PluginId,
};
use audiodown_plugin_api::manifest::CredentialTargetOrigin;
use chrono::{DateTime, Utc};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::{
    decrypt, encrypt, secret, CookieCredentialSecret, CredentialSecretGuard, CryptoError,
    EncryptedEnvelope, EncryptionContext, MasterKey, SecretPayloadError, TokenCredentialSecret,
};

const CURRENT_KEY_VERSION: u32 = 1;

#[async_trait]
pub trait CredentialRepository: Clone + Send + Sync + 'static {
    async fn insert(&self, record: &StoredCredential) -> Result<(), CredentialRepositoryError>;

    async fn update(
        &self,
        record: &StoredCredential,
        expected_revision: u64,
    ) -> Result<u64, CredentialRepositoryError>;

    async fn get(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<StoredCredential>, CredentialRepositoryError>;

    async fn list(&self) -> Result<Vec<StoredCredential>, CredentialRepositoryError>;

    async fn delete(&self, credential_id: &CredentialId) -> Result<(), CredentialRepositoryError>;

    async fn clear_source_plugin(
        &self,
        credential_id: &CredentialId,
    ) -> Result<(), CredentialRepositoryError>;
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRepositoryError {
    #[error("credential repository conflict")]
    Conflict,
    #[error("credential was not found")]
    NotFound,
    #[error("credential repository is unavailable")]
    Unavailable,
    #[error("credential repository rejected the record")]
    InvalidRecord,
}

#[derive(Clone)]
pub struct CredentialVault<R>
where
    R: CredentialRepository,
{
    inner: Arc<VaultInner<R>>,
}

impl<R> CredentialVault<R>
where
    R: CredentialRepository,
{
    pub fn new(master_key: MasterKey, repository: R) -> Self {
        Self {
            inner: Arc::new(VaultInner {
                master_key,
                repository,
            }),
        }
    }

    pub fn metadata(&self) -> MetadataPort<R> {
        MetadataPort {
            inner: self.inner.clone(),
        }
    }

    pub fn trusted(&self) -> TrustedPort<R> {
        TrustedPort {
            inner: self.inner.clone(),
        }
    }

    pub fn secrets(&self) -> SecretsPort<R> {
        SecretsPort {
            inner: self.inner.clone(),
        }
    }
}

struct VaultInner<R>
where
    R: CredentialRepository,
{
    master_key: MasterKey,
    repository: R,
}

#[derive(Clone)]
pub struct MetadataPort<R>
where
    R: CredentialRepository,
{
    inner: Arc<VaultInner<R>>,
}

impl<R> MetadataPort<R>
where
    R: CredentialRepository,
{
    pub async fn list(&self) -> Result<Vec<CredentialMetadata>, VaultError> {
        let mut metadata = self
            .inner
            .repository
            .list()
            .await
            .map_err(VaultError::from_repository)?
            .into_iter()
            .map(|record| record.to_metadata())
            .collect::<Vec<_>>();
        metadata.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then(left.id.as_uuid().cmp(&right.id.as_uuid()))
        });
        Ok(metadata)
    }

    pub async fn get(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<CredentialMetadata>, VaultError> {
        Ok(self
            .inner
            .repository
            .get(credential_id)
            .await
            .map_err(VaultError::from_repository)?
            .map(|record| record.to_metadata()))
    }

    pub async fn retain(
        &self,
        credential_id: &CredentialId,
    ) -> Result<CredentialMetadata, VaultError> {
        self.inner
            .repository
            .clear_source_plugin(credential_id)
            .await
            .map_err(VaultError::from_repository)?;
        self.get(credential_id).await?.ok_or(VaultError::NotFound)
    }

    pub async fn delete(&self, credential_id: &CredentialId) -> Result<(), VaultError> {
        self.inner
            .repository
            .delete(credential_id)
            .await
            .map_err(VaultError::from_repository)
    }
}

#[derive(Clone)]
pub struct TrustedPort<R>
where
    R: CredentialRepository,
{
    inner: Arc<VaultInner<R>>,
}

impl<R> TrustedPort<R>
where
    R: CredentialRepository,
{
    pub async fn create_cookie(
        &self,
        request: CredentialCreateRequest,
        secret: CookieCredentialSecret,
    ) -> Result<StoredCredential, VaultError> {
        self.create(request, CredentialKind::Cookie, SecretInput::Cookie(secret))
            .await
    }

    pub async fn create_token(
        &self,
        request: CredentialCreateRequest,
        secret: TokenCredentialSecret,
    ) -> Result<StoredCredential, VaultError> {
        self.create(request, CredentialKind::Token, SecretInput::Token(secret))
            .await
    }

    pub async fn update_cookie(
        &self,
        request: CredentialUpdateRequest,
        secret: CookieCredentialSecret,
    ) -> Result<StoredCredential, VaultError> {
        self.update(request, CredentialKind::Cookie, SecretInput::Cookie(secret))
            .await
    }

    pub async fn update_token(
        &self,
        request: CredentialUpdateRequest,
        secret: TokenCredentialSecret,
    ) -> Result<StoredCredential, VaultError> {
        self.update(request, CredentialKind::Token, SecretInput::Token(secret))
            .await
    }

    async fn create(
        &self,
        request: CredentialCreateRequest,
        kind: CredentialKind,
        secret: SecretInput,
    ) -> Result<StoredCredential, VaultError> {
        request.validate()?;
        validate_secret_origins(&request.target_origins, &secret)?;
        let id = CredentialId::from_uuid(Uuid::new_v4()).map_err(|_| VaultError::InvalidRequest)?;
        let now = Utc::now();
        let envelope = self.encrypt_secret(id, &request.scope, &secret)?;
        let metadata = CredentialMetadata {
            id,
            kind,
            platform_id: request.platform_id.clone(),
            scope: request.scope.clone(),
            ownership: request
                .source_plugin_id
                .clone()
                .map(CredentialOwnership::Plugin)
                .unwrap_or(CredentialOwnership::Retained),
            target_origins: request.target_origins.clone(),
            account_id_hint: request.account_id_hint,
            display_name: request.display_name,
            status: status_with_expiry(CredentialStatus::Active, request.expires_at),
            safe_error_summary: None,
            expires_at: request.expires_at,
            status_checked_at: Some(now),
            created_at: now,
            updated_at: now,
            revision: 1,
        };
        let record = StoredCredential {
            id,
            kind,
            platform_id: request.platform_id,
            scope: request.scope,
            source_plugin_id: request.source_plugin_id,
            target_origins: request.target_origins,
            account_id_hint: metadata.account_id_hint.clone(),
            display_name: metadata.display_name.clone(),
            status: metadata.status,
            safe_error_summary: None,
            expires_at: metadata.expires_at,
            status_checked_at: metadata.status_checked_at,
            created_at: now,
            updated_at: now,
            revision: 1,
            metadata,
            envelope,
        };
        self.inner
            .repository
            .insert(&record)
            .await
            .map_err(VaultError::from_repository)?;
        Ok(record)
    }

    async fn update(
        &self,
        request: CredentialUpdateRequest,
        kind: CredentialKind,
        secret: SecretInput,
    ) -> Result<StoredCredential, VaultError> {
        request.validate()?;
        validate_secret_origins(&request.target_origins, &secret)?;
        let existing = self
            .inner
            .repository
            .get(&request.credential_id)
            .await
            .map_err(VaultError::from_repository)?
            .ok_or(VaultError::NotFound)?;
        if existing.kind != kind || existing.revision != request.expected_revision {
            return Err(VaultError::Conflict);
        }

        let now = Utc::now();
        let envelope = self.encrypt_secret(existing.id, &existing.scope, &secret)?;
        let mut updated = existing;
        updated.target_origins = request.target_origins;
        updated.account_id_hint = request.account_id_hint;
        updated.display_name = request.display_name;
        updated.status = status_with_expiry(request.status, request.expires_at);
        updated.safe_error_summary = request.safe_error_summary;
        updated.expires_at = request.expires_at;
        updated.status_checked_at = request.status_checked_at;
        updated.updated_at = now;
        updated.envelope = envelope;
        updated.metadata = updated.to_metadata();

        let revision = self
            .inner
            .repository
            .update(&updated, request.expected_revision)
            .await
            .map_err(VaultError::from_repository)?;
        updated.revision = revision;
        updated.metadata.revision = revision;
        Ok(updated)
    }

    fn encrypt_secret(
        &self,
        credential_id: CredentialId,
        scope: &CredentialScope,
        secret: &SecretInput,
    ) -> Result<EncryptedEnvelope, VaultError> {
        let plaintext = match secret {
            SecretInput::Cookie(secret) => {
                secret::encode_cookie(secret).map_err(VaultError::from_secret)?
            }
            SecretInput::Token(secret) => {
                secret::encode_token(secret).map_err(VaultError::from_secret)?
            }
        };
        let context = EncryptionContext::new(credential_id, scope.clone(), CURRENT_KEY_VERSION);
        encrypt(&self.inner.master_key, &context, &plaintext).map_err(VaultError::from_crypto)
    }
}

#[derive(Clone)]
pub struct SecretsPort<R>
where
    R: CredentialRepository,
{
    inner: Arc<VaultInner<R>>,
}

impl<R> SecretsPort<R>
where
    R: CredentialRepository,
{
    pub async fn open(
        &self,
        credential_id: &CredentialId,
    ) -> Result<CredentialSecretGuard, VaultError> {
        let record = self
            .inner
            .repository
            .get(credential_id)
            .await
            .map_err(VaultError::from_repository)?
            .ok_or(VaultError::NotFound)?;
        if status_with_expiry(record.status, record.expires_at) == CredentialStatus::Expired {
            return Err(VaultError::Expired);
        }
        if record.status == CredentialStatus::Revoked {
            return Err(VaultError::Revoked);
        }
        let context = EncryptionContext::new(
            record.id,
            record.scope.clone(),
            record.envelope.key_version(),
        );
        let plaintext = decrypt(&self.inner.master_key, &context, &record.envelope)
            .map_err(VaultError::from_crypto)?;
        secret::decode(record.kind, &plaintext).map_err(VaultError::from_secret)
    }
}

#[derive(Debug, Clone)]
pub struct CredentialCreateRequest {
    pub platform_id: String,
    pub scope: CredentialScope,
    pub source_plugin_id: Option<PluginId>,
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub account_id_hint: Option<String>,
    pub display_name: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl CredentialCreateRequest {
    fn validate(&self) -> Result<(), VaultError> {
        validate_platform_id(&self.platform_id)?;
        validate_origins(&self.target_origins)?;
        validate_safe_text(self.account_id_hint.as_deref())?;
        validate_safe_text(self.display_name.as_deref())?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CredentialUpdateRequest {
    pub credential_id: CredentialId,
    pub expected_revision: u64,
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub account_id_hint: Option<String>,
    pub display_name: Option<String>,
    pub status: CredentialStatus,
    pub safe_error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status_checked_at: Option<DateTime<Utc>>,
}

impl CredentialUpdateRequest {
    fn validate(&self) -> Result<(), VaultError> {
        if self.expected_revision == 0 {
            return Err(VaultError::InvalidRequest);
        }
        validate_origins(&self.target_origins)?;
        validate_safe_text(self.account_id_hint.as_deref())?;
        validate_safe_text(self.display_name.as_deref())?;
        validate_safe_text(self.safe_error_summary.as_deref())?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct StoredCredential {
    pub metadata: CredentialMetadata,
    pub id: CredentialId,
    pub kind: CredentialKind,
    pub platform_id: String,
    pub scope: CredentialScope,
    pub source_plugin_id: Option<PluginId>,
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub account_id_hint: Option<String>,
    pub display_name: Option<String>,
    pub status: CredentialStatus,
    pub safe_error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revision: u64,
    pub envelope: EncryptedEnvelope,
}

impl StoredCredential {
    pub fn to_metadata(&self) -> CredentialMetadata {
        CredentialMetadata {
            id: self.id,
            kind: self.kind,
            platform_id: self.platform_id.clone(),
            scope: self.scope.clone(),
            ownership: self
                .source_plugin_id
                .clone()
                .map(CredentialOwnership::Plugin)
                .unwrap_or(CredentialOwnership::Retained),
            target_origins: self.target_origins.clone(),
            account_id_hint: self.account_id_hint.clone(),
            display_name: self.display_name.clone(),
            status: status_with_expiry(self.status, self.expires_at),
            safe_error_summary: self.safe_error_summary.clone(),
            expires_at: self.expires_at,
            status_checked_at: self.status_checked_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
            revision: self.revision,
        }
    }
}

impl fmt::Debug for StoredCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredCredential")
            .field("id", &self.id)
            .field("metadata", &self.metadata)
            .field("kind", &self.kind)
            .field("platform_id", &self.platform_id)
            .field("scope", &self.scope)
            .field("source_plugin_id", &self.source_plugin_id)
            .field("target_origins", &self.target_origins)
            .field("account_id_hint", &self.account_id_hint)
            .field("display_name", &self.display_name)
            .field("status", &self.status)
            .field("safe_error_summary", &self.safe_error_summary)
            .field("expires_at", &self.expires_at)
            .field("status_checked_at", &self.status_checked_at)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("revision", &self.revision)
            .field("envelope", &self.envelope)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMetadata {
    pub id: CredentialId,
    pub kind: CredentialKind,
    pub platform_id: String,
    pub scope: CredentialScope,
    pub ownership: CredentialOwnership,
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub account_id_hint: Option<String>,
    pub display_name: Option<String>,
    pub status: CredentialStatus,
    pub safe_error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revision: u64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    #[error("credential request is invalid")]
    InvalidRequest,
    #[error("credential was not found")]
    NotFound,
    #[error("credential update conflicted")]
    Conflict,
    #[error("credential repository is unavailable")]
    RepositoryUnavailable,
    #[error("credential has expired")]
    Expired,
    #[error("credential has been revoked")]
    Revoked,
    #[error("credential encryption failed")]
    Crypto,
    #[error("credential secret payload is invalid")]
    SecretPayload,
}

impl VaultError {
    fn from_repository(error: CredentialRepositoryError) -> Self {
        match error {
            CredentialRepositoryError::Conflict => Self::Conflict,
            CredentialRepositoryError::NotFound => Self::NotFound,
            CredentialRepositoryError::Unavailable => Self::RepositoryUnavailable,
            CredentialRepositoryError::InvalidRecord => Self::InvalidRequest,
        }
    }

    fn from_crypto(_error: CryptoError) -> Self {
        Self::Crypto
    }

    fn from_secret(_error: SecretPayloadError) -> Self {
        Self::SecretPayload
    }
}

enum SecretInput {
    Cookie(CookieCredentialSecret),
    Token(TokenCredentialSecret),
}

fn status_with_expiry(
    status: CredentialStatus,
    expires_at: Option<DateTime<Utc>>,
) -> CredentialStatus {
    if status == CredentialStatus::Active
        && expires_at.is_some_and(|expires_at| expires_at <= Utc::now())
    {
        CredentialStatus::Expired
    } else {
        status
    }
}

fn validate_platform_id(platform_id: &str) -> Result<(), VaultError> {
    if platform_id.is_empty()
        || platform_id.len() > 64
        || !platform_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(VaultError::InvalidRequest);
    }
    Ok(())
}

fn validate_origins(origins: &[CredentialTargetOrigin]) -> Result<(), VaultError> {
    if origins.is_empty() || origins.len() > 16 {
        return Err(VaultError::InvalidRequest);
    }
    for origin in origins {
        CredentialTargetOrigin::parse(origin.as_str()).map_err(|_| VaultError::InvalidRequest)?;
    }
    Ok(())
}

fn validate_safe_text(value: Option<&str>) -> Result<(), VaultError> {
    if value.is_some_and(|value| value.len() > 512 || value.chars().any(char::is_control)) {
        return Err(VaultError::InvalidRequest);
    }
    Ok(())
}

fn validate_secret_origins(
    origins: &[CredentialTargetOrigin],
    secret: &SecretInput,
) -> Result<(), VaultError> {
    if let SecretInput::Cookie(secret) = secret {
        for cookie in secret.cookies() {
            let matches_origin = origins
                .iter()
                .filter_map(|origin| origin_host(origin).ok())
                .any(|host| host.eq_ignore_ascii_case(cookie.host()));
            if !matches_origin {
                return Err(VaultError::InvalidRequest);
            }
        }
    }
    Ok(())
}

fn origin_host(origin: &CredentialTargetOrigin) -> Result<String, VaultError> {
    Url::parse(origin.as_str())
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .ok_or(VaultError::InvalidRequest)
}
