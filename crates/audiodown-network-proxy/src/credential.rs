use async_trait::async_trait;
use audiodown_credential_vault::{
    CookieCredentialSecret, CredentialMetadata, CredentialSecretGuard, CredentialUpdateRequest,
};
use audiodown_domain::{
    credential::{CredentialId, CredentialScope},
    plugin::PluginId,
};
use audiodown_plugin_api::manifest::{CredentialTargetOrigin, PluginManifest};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct InstalledPluginContext {
    pub plugin_id: PluginId,
    pub manifest_hash: String,
    pub manifest: PluginManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSelection {
    pub credential_id: CredentialId,
    pub scope: CredentialScope,
}

pub struct OpenedCredential {
    pub metadata: CredentialMetadata,
    pub secret: CredentialSecretGuard,
}

impl std::fmt::Debug for OpenedCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedCredential")
            .field("metadata", &self.metadata)
            .field("secret", &self.secret)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveGrantSnapshot {
    pub plugin_id: PluginId,
    pub manifest_hash: String,
    pub credential_id: CredentialId,
    pub scope: CredentialScope,
    pub target_origins: Vec<CredentialTargetOrigin>,
}

#[async_trait]
pub trait CredentialVaultPort: Clone + Send + Sync + 'static {
    /// Opens metadata and its secret from one current credential revision.
    async fn open_current(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<OpenedCredential>, CredentialPortError>;

    async fn replace_cookie(
        &self,
        request: CredentialUpdateRequest,
        secret: CookieCredentialSecret,
    ) -> Result<CredentialMetadata, CredentialPortError>;
}

#[async_trait]
pub trait CredentialGrantPort: Clone + Send + Sync + 'static {
    async fn active_grant(
        &self,
        plugin_id: &PluginId,
        credential_id: &CredentialId,
        scope: &CredentialScope,
    ) -> Result<Option<ActiveGrantSnapshot>, CredentialPortError>;
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CredentialPortError {
    #[error("credential was not found")]
    NotFound,
    #[error("credential update conflicted")]
    Conflict,
    #[error("credential has expired")]
    Expired,
    #[error("credential has been revoked")]
    Revoked,
    #[error("credential port is unavailable")]
    Unavailable,
}
