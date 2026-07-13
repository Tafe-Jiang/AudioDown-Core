use std::{
    collections::HashMap,
    net::{IpAddr, ToSocketAddrs},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use audiodown_credential_vault::{
    CookieCredentialSecret, CredentialMetadata, CredentialRepository, CredentialRepositoryError,
    CredentialUpdateRequest, CredentialVault, EncryptedEnvelope, StoredCredential, VaultError,
};
use audiodown_domain::{
    credential::{CredentialOwnership, CredentialScope},
    plugin::{PluginId, PluginStatus},
};
use audiodown_network_proxy::{
    credential::{
        ActiveGrantSnapshot, CredentialGrantPort, CredentialPortError, CredentialSelection,
        CredentialVaultPort, InstalledPluginContext, OpenedCredential,
    },
    error::HttpProxyError,
    http::{HttpProxy, HttpTransport, ProxyRequest, ReqwestTransport},
    policy::ProxyPolicy,
    resolver::{DnsResolver, ResolveError},
    service::{CredentialProxyError, CredentialProxyRequest, CredentialProxyService},
};
use audiodown_plugin_api::manifest::PluginManifest;
use audiodown_storage::{CredentialRecord, Storage, StorageError};
use thiserror::Error;

use crate::proxy_gateway::{
    AuthenticatedRuntime, CoreProxyBackend, CoreProxyBackendError, CoreProxyRequest,
    CoreProxyResponse,
};

#[derive(Clone)]
pub struct SqliteVaultRepository {
    storage: Storage,
}

impl SqliteVaultRepository {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl CredentialRepository for SqliteVaultRepository {
    async fn insert(&self, record: &StoredCredential) -> Result<(), CredentialRepositoryError> {
        let record = stored_to_record(record)?;
        self.storage
            .credentials()
            .insert(&record)
            .await
            .map_err(map_repository_error)
    }

    async fn update(
        &self,
        record: &StoredCredential,
        expected_revision: u64,
    ) -> Result<u64, CredentialRepositoryError> {
        if record.revision != expected_revision {
            return Err(CredentialRepositoryError::Conflict);
        }
        let record = stored_to_record(record)?;
        self.storage
            .credentials()
            .upsert(&record)
            .await
            .map_err(map_repository_error)
    }

    async fn get(
        &self,
        credential_id: &audiodown_domain::credential::CredentialId,
    ) -> Result<Option<StoredCredential>, CredentialRepositoryError> {
        self.storage
            .credentials()
            .get(credential_id)
            .await
            .map_err(map_repository_error)?
            .map(record_to_stored)
            .transpose()
    }

    async fn list(&self) -> Result<Vec<StoredCredential>, CredentialRepositoryError> {
        self.storage
            .credentials()
            .list()
            .await
            .map_err(map_repository_error)?
            .into_iter()
            .map(record_to_stored)
            .collect()
    }

    async fn delete(
        &self,
        credential_id: &audiodown_domain::credential::CredentialId,
    ) -> Result<(), CredentialRepositoryError> {
        self.storage
            .credentials()
            .delete(credential_id)
            .await
            .map_err(map_repository_error)
    }

    async fn clear_source_plugin(
        &self,
        credential_id: &audiodown_domain::credential::CredentialId,
    ) -> Result<(), CredentialRepositoryError> {
        self.storage
            .credentials()
            .clear_source_plugin(credential_id)
            .await
            .map_err(map_repository_error)
    }
}

fn stored_to_record(
    record: &StoredCredential,
) -> Result<CredentialRecord, CredentialRepositoryError> {
    if record.metadata != record.to_metadata() {
        return Err(CredentialRepositoryError::InvalidRecord);
    }
    Ok(CredentialRecord {
        id: record.id,
        kind: record.kind,
        platform_id: record.platform_id.clone(),
        scope: record.scope.clone(),
        source_plugin_id: record.source_plugin_id.clone(),
        algorithm_version: record.envelope.algorithm_version(),
        key_version: record.envelope.key_version(),
        nonce: *record.envelope.nonce(),
        ciphertext: record.envelope.ciphertext().to_vec(),
        target_origins: record.target_origins.clone(),
        status: record.status,
        account_id_hint: record.account_id_hint.clone(),
        display_name: record.display_name.clone(),
        safe_error_summary: record.safe_error_summary.clone(),
        expires_at: record.expires_at,
        status_checked_at: record.status_checked_at,
        revision: record.revision,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

fn record_to_stored(
    record: CredentialRecord,
) -> Result<StoredCredential, CredentialRepositoryError> {
    let metadata = CredentialMetadata {
        id: record.id,
        kind: record.kind,
        platform_id: record.platform_id.clone(),
        scope: record.scope.clone(),
        ownership: record
            .source_plugin_id
            .clone()
            .map(CredentialOwnership::Plugin)
            .unwrap_or(CredentialOwnership::Retained),
        target_origins: record.target_origins.clone(),
        account_id_hint: record.account_id_hint.clone(),
        display_name: record.display_name.clone(),
        status: record.status,
        safe_error_summary: record.safe_error_summary.clone(),
        expires_at: record.expires_at,
        status_checked_at: record.status_checked_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
        revision: record.revision,
    };
    Ok(StoredCredential {
        metadata,
        id: record.id,
        kind: record.kind,
        platform_id: record.platform_id,
        scope: record.scope,
        source_plugin_id: record.source_plugin_id,
        target_origins: record.target_origins,
        account_id_hint: record.account_id_hint,
        display_name: record.display_name,
        status: record.status,
        safe_error_summary: record.safe_error_summary,
        expires_at: record.expires_at,
        status_checked_at: record.status_checked_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
        revision: record.revision,
        envelope: EncryptedEnvelope::from_parts(
            record.algorithm_version,
            record.key_version,
            record.nonce,
            record.ciphertext,
        ),
    })
}

fn map_repository_error(error: StorageError) -> CredentialRepositoryError {
    match error {
        StorageError::Conflict => CredentialRepositoryError::Conflict,
        StorageError::NotFound => CredentialRepositoryError::NotFound,
        StorageError::InvalidData(_) => CredentialRepositoryError::InvalidRecord,
        StorageError::Database(_) | StorageError::Migration(_) => {
            CredentialRepositoryError::Unavailable
        }
    }
}

#[derive(Clone)]
pub struct SqliteCredentialVaultPort {
    vault: CredentialVault<SqliteVaultRepository>,
}

impl SqliteCredentialVaultPort {
    pub fn new(vault: CredentialVault<SqliteVaultRepository>) -> Self {
        Self { vault }
    }
}

#[async_trait]
impl CredentialVaultPort for SqliteCredentialVaultPort {
    async fn open_current(
        &self,
        credential_id: &audiodown_domain::credential::CredentialId,
    ) -> Result<Option<OpenedCredential>, CredentialPortError> {
        let Some(before) = self
            .vault
            .metadata()
            .get(credential_id)
            .await
            .map_err(map_vault_error)?
        else {
            return Ok(None);
        };
        let secret = self
            .vault
            .secrets()
            .open(credential_id)
            .await
            .map_err(map_vault_error)?;
        let after = self
            .vault
            .metadata()
            .get(credential_id)
            .await
            .map_err(map_vault_error)?
            .ok_or(CredentialPortError::NotFound)?;
        if before != after {
            return Err(CredentialPortError::Conflict);
        }
        Ok(Some(OpenedCredential {
            metadata: after,
            secret,
        }))
    }

    async fn replace_cookie(
        &self,
        request: CredentialUpdateRequest,
        secret: CookieCredentialSecret,
    ) -> Result<CredentialMetadata, CredentialPortError> {
        self.vault
            .trusted()
            .update_cookie(request, secret)
            .await
            .map(|record| record.metadata)
            .map_err(map_vault_error)
    }
}

fn map_vault_error(error: VaultError) -> CredentialPortError {
    match error {
        VaultError::NotFound => CredentialPortError::NotFound,
        VaultError::Conflict => CredentialPortError::Conflict,
        VaultError::Expired => CredentialPortError::Expired,
        VaultError::Revoked => CredentialPortError::Revoked,
        VaultError::InvalidRequest
        | VaultError::RepositoryUnavailable
        | VaultError::Crypto
        | VaultError::SecretPayload => CredentialPortError::Unavailable,
    }
}

#[derive(Clone)]
pub struct SqliteCredentialGrantPort {
    storage: Storage,
}

impl SqliteCredentialGrantPort {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl CredentialGrantPort for SqliteCredentialGrantPort {
    async fn active_grant(
        &self,
        plugin_id: &PluginId,
        credential_id: &audiodown_domain::credential::CredentialId,
        scope: &CredentialScope,
    ) -> Result<Option<ActiveGrantSnapshot>, CredentialPortError> {
        self.storage
            .credentials()
            .active_grant(plugin_id, credential_id, scope)
            .await
            .map_err(|_| CredentialPortError::Unavailable)
            .map(|grant| {
                grant.map(|grant| ActiveGrantSnapshot {
                    plugin_id: grant.plugin_id,
                    manifest_hash: grant.manifest_hash,
                    credential_id: grant.credential_id,
                    scope: grant.scope,
                    target_origins: grant.target_origins,
                })
            })
    }
}

#[derive(Clone)]
pub struct SqliteInstalledPluginLoader {
    storage: Storage,
}

impl SqliteInstalledPluginLoader {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub async fn load(
        &self,
        plugin_id: &PluginId,
    ) -> Result<InstalledPluginContext, ProxyAdapterError> {
        let record = self
            .storage
            .plugins()
            .get(plugin_id)
            .await
            .map_err(|_| ProxyAdapterError::Unavailable)?
            .ok_or(ProxyAdapterError::NotFound)?;
        if !record.enabled || record.status != PluginStatus::Healthy {
            return Err(ProxyAdapterError::Inactive);
        }
        let manifest: PluginManifest = serde_json::from_value(record.manifest_json.clone())
            .map_err(|_| ProxyAdapterError::InvalidManifest)?;
        if manifest.id != record.plugin_id
            || manifest.plugin_type != record.plugin_type
            || manifest.platform.id != record.platform_id
            || manifest.version.to_string() != record.version
            || !valid_manifest_hash(&record.manifest_hash)
        {
            return Err(ProxyAdapterError::InvalidManifest);
        }
        Ok(InstalledPluginContext {
            plugin_id: record.plugin_id,
            manifest_hash: record.manifest_hash,
            manifest,
        })
    }
}

fn valid_manifest_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[derive(Clone)]
pub struct SqliteCredentialSelector {
    storage: Storage,
}

impl SqliteCredentialSelector {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub async fn select(
        &self,
        scope: &CredentialScope,
    ) -> Result<Option<CredentialSelection>, ProxyAdapterError> {
        self.storage
            .credentials()
            .get_by_scope(scope)
            .await
            .map_err(|_| ProxyAdapterError::Unavailable)
            .map(|record| {
                record.map(|record| CredentialSelection {
                    credential_id: record.id,
                    scope: scope.clone(),
                })
            })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDnsResolver;

impl DnsResolver for SystemDnsResolver {
    fn resolve(&mut self, host: &str) -> Result<Vec<IpAddr>, ResolveError> {
        let mut addresses = (host, 0)
            .to_socket_addrs()
            .map_err(|_| ResolveError::NotFound)?
            .map(|address| address.ip())
            .collect::<Vec<_>>();
        addresses.sort();
        addresses.dedup();
        if addresses.is_empty() {
            Err(ResolveError::NotFound)
        } else {
            Ok(addresses)
        }
    }
}

type ProxyService<R, T> =
    CredentialProxyService<R, T, SqliteCredentialVaultPort, SqliteCredentialGrantPort>;
type ServiceCache<R, T> = HashMap<(PluginId, String), Arc<ProxyService<R, T>>>;

pub struct SqliteCoreProxyBackend<R = SystemDnsResolver, T = ReqwestTransport> {
    loader: SqliteInstalledPluginLoader,
    selector: SqliteCredentialSelector,
    vault: SqliteCredentialVaultPort,
    grants: SqliteCredentialGrantPort,
    resolver: R,
    transport: T,
    services: Mutex<ServiceCache<R, T>>,
}

impl<R, T> SqliteCoreProxyBackend<R, T>
where
    R: DnsResolver + Clone + Send + Sync + 'static,
    T: HttpTransport + Clone,
{
    pub fn new(
        storage: Storage,
        vault: CredentialVault<SqliteVaultRepository>,
        resolver: R,
        transport: T,
    ) -> Self {
        Self {
            loader: SqliteInstalledPluginLoader::new(storage.clone()),
            selector: SqliteCredentialSelector::new(storage.clone()),
            vault: SqliteCredentialVaultPort::new(vault),
            grants: SqliteCredentialGrantPort::new(storage),
            resolver,
            transport,
            services: Mutex::new(HashMap::new()),
        }
    }

    pub fn service_for(
        &self,
        plugin: &InstalledPluginContext,
    ) -> Result<Arc<ProxyService<R, T>>, ProxyAdapterError> {
        let key = (plugin.plugin_id.clone(), plugin.manifest_hash.clone());
        let mut services = self
            .services
            .lock()
            .map_err(|_| ProxyAdapterError::Unavailable)?;
        if let Some(service) = services.get(&key) {
            return Ok(Arc::clone(service));
        }
        services.retain(|(plugin_id, _), _| plugin_id != &plugin.plugin_id);
        let service = Arc::new(CredentialProxyService::new(
            HttpProxy::new(
                ProxyPolicy::production(&plugin.manifest),
                self.resolver.clone(),
                self.transport.clone(),
            ),
            self.vault.clone(),
            self.grants.clone(),
        ));
        services.insert(key, Arc::clone(&service));
        Ok(service)
    }
}

impl SqliteCoreProxyBackend<SystemDnsResolver, ReqwestTransport> {
    pub fn production(storage: Storage, vault: CredentialVault<SqliteVaultRepository>) -> Self {
        Self::new(storage, vault, SystemDnsResolver, ReqwestTransport)
    }
}

#[async_trait]
impl<R, T> CoreProxyBackend for SqliteCoreProxyBackend<R, T>
where
    R: DnsResolver + Clone + Send + Sync + 'static,
    T: HttpTransport + Clone,
{
    async fn execute(
        &self,
        runtime: &AuthenticatedRuntime,
        request: CoreProxyRequest,
    ) -> Result<CoreProxyResponse, CoreProxyBackendError> {
        let plugin = self
            .loader
            .load(runtime.plugin_id())
            .await
            .map_err(map_adapter_error)?;
        let service = self.service_for(&plugin).map_err(map_adapter_error)?;
        let credential = match &request.credential_scope {
            Some(scope) => self
                .selector
                .select(scope)
                .await
                .map_err(map_adapter_error)?
                .ok_or(CoreProxyBackendError::CredentialScopeNotAllowed)?
                .into(),
            None => None,
        };
        let response = service
            .execute(
                &plugin,
                CredentialProxyRequest {
                    request: ProxyRequest {
                        method: request.method,
                        url: request.url,
                        headers: request.headers,
                        body: request.body,
                    },
                    cookie_jar_session_id: request.cookie_jar_session_id,
                    credential,
                },
            )
            .await
            .map_err(map_proxy_error)?;
        Ok(CoreProxyResponse::new(
            response.status,
            response.headers,
            response.body,
        ))
    }
}

fn map_adapter_error(error: ProxyAdapterError) -> CoreProxyBackendError {
    match error {
        ProxyAdapterError::NotFound
        | ProxyAdapterError::Inactive
        | ProxyAdapterError::InvalidManifest => CoreProxyBackendError::PolicyDenied,
        ProxyAdapterError::Unavailable => CoreProxyBackendError::Unavailable,
    }
}

fn map_proxy_error(error: CredentialProxyError) -> CoreProxyBackendError {
    match error {
        CredentialProxyError::InvalidRequest => CoreProxyBackendError::InvalidRequest,
        CredentialProxyError::ScopeNotDeclared => CoreProxyBackendError::CredentialScopeNotAllowed,
        CredentialProxyError::RefreshConflict => CoreProxyBackendError::Busy,
        CredentialProxyError::DirectReflection => CoreProxyBackendError::ResponseRejected,
        CredentialProxyError::SecretUnavailable | CredentialProxyError::Unavailable => {
            CoreProxyBackendError::Unavailable
        }
        CredentialProxyError::Proxy(HttpProxyError::Timeout) => CoreProxyBackendError::Timeout,
        CredentialProxyError::Proxy(HttpProxyError::ConcurrencyLimited) => {
            CoreProxyBackendError::Busy
        }
        CredentialProxyError::Proxy(
            HttpProxyError::ResponseHeadersTooLarge
            | HttpProxyError::ResponseBodyTooLarge
            | HttpProxyError::InvalidResponseEncoding,
        ) => CoreProxyBackendError::ResponseRejected,
        CredentialProxyError::Proxy(HttpProxyError::Transport) => {
            CoreProxyBackendError::Unavailable
        }
        CredentialProxyError::Proxy(
            HttpProxyError::MethodNotAllowed
            | HttpProxyError::RequestHeadersTooLarge
            | HttpProxyError::RequestBodyTooLarge,
        ) => CoreProxyBackendError::InvalidRequest,
        CredentialProxyError::JarNotFound
        | CredentialProxyError::JarExpired
        | CredentialProxyError::JarBindingMismatch
        | CredentialProxyError::JarPurposeMismatch
        | CredentialProxyError::CredentialNotFound
        | CredentialProxyError::CredentialExpired
        | CredentialProxyError::CredentialRevoked
        | CredentialProxyError::CredentialMismatch
        | CredentialProxyError::GrantMissing
        | CredentialProxyError::GrantMismatch
        | CredentialProxyError::OriginDenied
        | CredentialProxyError::RequestRejected
        | CredentialProxyError::Proxy(HttpProxyError::Policy(_))
        | CredentialProxyError::Proxy(HttpProxyError::TooManyRedirects)
        | CredentialProxyError::Proxy(HttpProxyError::InvalidRedirect)
        | CredentialProxyError::Proxy(HttpProxyError::RequestRejected) => {
            CoreProxyBackendError::PolicyDenied
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProxyAdapterError {
    #[error("installed plugin was not found")]
    NotFound,
    #[error("installed plugin is not active")]
    Inactive,
    #[error("installed plugin manifest is invalid")]
    InvalidManifest,
    #[error("proxy adapter is unavailable")]
    Unavailable,
}
