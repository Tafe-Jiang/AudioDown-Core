use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write},
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use audiodown_credential_vault::{
    CookieCredentialSecret, CookieSecretRecord, CredentialCreateRequest, CredentialMetadata,
    CredentialRepository, CredentialRepositoryError, CredentialSecretGuard,
    CredentialUpdateRequest, CredentialVault, MasterKey, StoredCredential, TokenCredentialSecret,
};
use audiodown_domain::{
    credential::{CredentialId, CredentialKind, CredentialScope, CredentialStatus},
    plugin::PluginId,
};
use audiodown_network_proxy::{
    credential::{
        ActiveGrantSnapshot, CredentialGrantPort, CredentialPortError, CredentialSelection,
        CredentialVaultPort, InstalledPluginContext, OpenedCredential,
    },
    http::{
        HttpProxy, HttpTransport, ProxyRequest, TransportError, TransportRequest, TransportResponse,
    },
    policy::ProxyPolicy,
    resolver::StaticResolver,
    service::{
        CredentialProxyError, CredentialProxyRequest, CredentialProxyService,
        ValidatedCredentialUpdate,
    },
};
use audiodown_plugin_api::manifest::{CredentialTargetOrigin, PluginManifest};
use chrono::Utc;
use flate2::{write::GzEncoder, Compression};
use http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use secrecy::{Secret, SecretString};
use tracing::instrument::WithSubscriber;

const HOST: &str = "api.virtual.invalid";
const ORIGIN: &str = "http://api.virtual.invalid:18080";
const MANIFEST_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COOKIE_CANARY: &str = "stored-cookie-canary-must-remain-secret";
const TOKEN_CANARY: &str = "stored-token-canary-must-remain-secret";

#[tokio::test]
async fn injects_cookie_only_after_complete_manifest_grant_and_credential_authorization() {
    let vault = TestVault::new();
    let record = vault.create_cookie(None).await;
    let grants = TestGrants::new(Some(grant_for(&record.metadata)));
    let transport = ScriptedTransport::with_responses([response_with_set_cookie(
        "upstream=hidden; Path=/",
        b"ok".to_vec(),
    )]);
    let service = service(transport.clone(), vault, grants);
    let plugin = content_plugin(MANIFEST_HASH, ORIGIN);

    let response = service
        .execute(
            &plugin,
            credential_request(record.id, "/resource?query=request-canary"),
        )
        .await
        .expect("authorized proxy response");

    assert_eq!(response.body, b"ok");
    assert!(!response.headers.contains_key(header::SET_COOKIE));
    let seen = transport.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].headers[header::COOKIE],
        format!("session={COOKIE_CANARY}")
    );
    assert!(!seen[0].headers.contains_key(header::AUTHORIZATION));
}

#[tokio::test]
async fn injects_trusted_bearer_fixture_and_rejects_direct_reflection_after_gzip_decode() {
    let vault = TestVault::new();
    let record = vault.create_token().await;
    let grants = TestGrants::new(Some(grant_for(&record.metadata)));
    let mut reflected = ok_response(gzip(format!("prefix {TOKEN_CANARY} suffix").as_bytes()));
    reflected
        .headers
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    let transport = ScriptedTransport::with_responses([reflected]);
    let service = service(transport.clone(), vault, grants);

    let error = service
        .execute(
            &content_plugin(MANIFEST_HASH, ORIGIN),
            credential_request(record.id, "/token"),
        )
        .await
        .unwrap_err();

    assert_eq!(error, CredentialProxyError::DirectReflection);
    assert_eq!(
        transport.seen()[0].headers[header::AUTHORIZATION],
        format!("Bearer {TOKEN_CANARY}")
    );
    let rendered = format!("{error:?}\n{error}");
    assert!(!rendered.contains(TOKEN_CANARY));
}

#[tokio::test]
async fn fails_closed_for_missing_stale_or_origin_mismatched_grants() {
    let vault = TestVault::new();
    let record = vault.create_cookie(None).await;
    let plugin = content_plugin(MANIFEST_HASH, ORIGIN);

    let missing = service(
        ScriptedTransport::default(),
        vault.clone(),
        TestGrants::new(None),
    );
    assert_eq!(
        missing
            .execute(&plugin, credential_request(record.id, "/missing"))
            .await,
        Err(CredentialProxyError::GrantMissing)
    );

    let mut stale = grant_for(&record.metadata);
    stale.manifest_hash = "b".repeat(64);
    let stale_service = service(
        ScriptedTransport::default(),
        vault.clone(),
        TestGrants::new(Some(stale)),
    );
    assert_eq!(
        stale_service
            .execute(&plugin, credential_request(record.id, "/stale"))
            .await,
        Err(CredentialProxyError::GrantMismatch)
    );

    let mut wrong_origin = grant_for(&record.metadata);
    wrong_origin.target_origins = origins(&["http://api.virtual.invalid:18081"]);
    let origin_service = service(
        ScriptedTransport::default(),
        vault,
        TestGrants::new(Some(wrong_origin)),
    );
    assert_eq!(
        origin_service
            .execute(&plugin, credential_request(record.id, "/origin"))
            .await,
        Err(CredentialProxyError::OriginDenied)
    );
}

#[tokio::test]
async fn rejects_undeclared_scope_expired_credential_and_kind_mismatch_before_transport() {
    let vault = TestVault::new();
    let active = vault.create_cookie(None).await;
    let transport = ScriptedTransport::default();
    let primary_service = service(
        transport.clone(),
        vault.clone(),
        TestGrants::new(Some(grant_for(&active.metadata))),
    );
    let undeclared = content_plugin(MANIFEST_HASH, "http://other.virtual.invalid:18080");
    assert_eq!(
        primary_service
            .execute(&undeclared, credential_request(active.id, "/undeclared"))
            .await,
        Err(CredentialProxyError::OriginDenied)
    );

    let expired = vault.create_cookie(Some(Utc::now())).await;
    let expired_service = service(
        transport.clone(),
        vault.clone(),
        TestGrants::new(Some(grant_for(&expired.metadata))),
    );
    assert_eq!(
        expired_service
            .execute(
                &content_plugin(MANIFEST_HASH, ORIGIN),
                credential_request(expired.id, "/expired"),
            )
            .await,
        Err(CredentialProxyError::CredentialExpired)
    );

    let token = vault.create_token().await;
    vault.force_metadata_kind(token.id, CredentialKind::Cookie);
    let mismatch = service(
        transport.clone(),
        vault,
        TestGrants::new(Some(grant_for(&token.metadata))),
    );
    assert_eq!(
        mismatch
            .execute(
                &content_plugin(MANIFEST_HASH, ORIGIN),
                credential_request(token.id, "/kind"),
            )
            .await,
        Err(CredentialProxyError::SecretUnavailable)
    );
    assert!(transport.seen().is_empty());
}

#[tokio::test]
async fn login_jar_captures_redirect_cookie_and_reauthorizes_every_hop() {
    let vault = TestVault::new();
    let grants = TestGrants::new(None);
    let transport = ScriptedTransport::with_responses([
        redirect_with_cookie("/next", "login=redirect-canary; Path=/"),
        ok_response(b"complete".to_vec()),
    ]);
    let service = service(transport.clone(), vault, grants);
    let plugin = credential_plugin(ORIGIN);
    let session = service
        .create_login_jar(
            &plugin,
            scope(),
            origins(&[ORIGIN]),
            Duration::from_secs(60),
        )
        .expect("login jar");

    let response = service
        .execute(&plugin, jar_request(session, "/login"))
        .await
        .expect("login proxy response");

    assert_eq!(response.body, b"complete");
    let seen = transport.seen();
    assert_eq!(seen.len(), 2);
    assert!(!seen[0].headers.contains_key(header::COOKIE));
    assert_eq!(seen[1].headers[header::COOKIE], "login=redirect-canary");
}

#[tokio::test]
async fn redirect_to_same_host_but_different_origin_is_denied_before_second_transport_call() {
    let transport = ScriptedTransport::with_responses([redirect_response(
        "http://api.virtual.invalid:18081/next",
    )]);
    let service = service(transport.clone(), TestVault::new(), TestGrants::new(None));
    let plugin = credential_plugin(ORIGIN);
    let session = service
        .create_login_jar(
            &plugin,
            scope(),
            origins(&[ORIGIN]),
            Duration::from_secs(60),
        )
        .unwrap();

    assert_eq!(
        service
            .execute(&plugin, jar_request(session, "/redirect"))
            .await,
        Err(CredentialProxyError::OriginDenied)
    );
    assert_eq!(transport.seen().len(), 1);
}

#[tokio::test]
async fn rejects_secret_reflection_in_visible_headers_but_allows_core_consumed_set_cookie() {
    let vault = TestVault::new();
    let record = vault.create_cookie(None).await;
    let mut reflected = ok_response(Vec::new());
    reflected.headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("session={COOKIE_CANARY}")).unwrap(),
    );
    reflected.headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("session={COOKIE_CANARY}; Path=/")).unwrap(),
    );
    let service = service(
        ScriptedTransport::with_responses([reflected]),
        vault,
        TestGrants::new(Some(grant_for(&record.metadata))),
    );

    assert_eq!(
        service
            .execute(
                &content_plugin(MANIFEST_HASH, ORIGIN),
                credential_request(record.id, "/reflect"),
            )
            .await,
        Err(CredentialProxyError::DirectReflection)
    );
}

#[tokio::test]
async fn refresh_jar_replaces_cookie_once_with_bound_revision_and_invalidates_on_conflict() {
    let vault = TestVault::new();
    let record = vault.create_cookie(None).await;
    let plugin = credential_plugin(ORIGIN);
    let transport = ScriptedTransport::with_responses([response_with_set_cookie(
        "session=refreshed-cookie-canary; Path=/",
        Vec::new(),
    )]);
    let refresh_service = service(transport.clone(), vault.clone(), TestGrants::new(None));
    let session = refresh_service
        .create_refresh_jar(&plugin, record.id, scope(), Duration::from_secs(60))
        .await
        .expect("refresh jar");

    refresh_service
        .execute(&plugin, jar_request(session, "/refresh"))
        .await
        .expect("refresh request");
    assert_eq!(
        transport.seen()[0].headers[header::COOKIE],
        format!("session={COOKIE_CANARY}")
    );

    let updated = refresh_service
        .commit_refresh(
            &plugin,
            &session,
            ValidatedCredentialUpdate {
                target_origins: origins(&[ORIGIN]),
                account_id_hint: Some("account-1".to_string()),
                display_name: Some("Virtual Account".to_string()),
                status: CredentialStatus::Active,
                safe_error_summary: None,
                expires_at: None,
                status_checked_at: Some(Utc::now()),
            },
        )
        .await
        .expect("commit refresh");
    assert_eq!(updated.revision, 2);
    let guard = vault.open_secret(record.id).await;
    assert_eq!(
        guard.cookie().unwrap().cookies()[0].with_value(str::to_owned),
        "refreshed-cookie-canary"
    );
    assert_eq!(
        refresh_service
            .commit_refresh(
                &plugin,
                &session,
                ValidatedCredentialUpdate::active(origins(&[ORIGIN])),
            )
            .await,
        Err(CredentialProxyError::JarNotFound)
    );

    let conflicted = vault.create_cookie(None).await;
    let conflict_service = service(
        ScriptedTransport::default(),
        vault.clone(),
        TestGrants::new(None),
    );
    let conflict_session = conflict_service
        .create_refresh_jar(&plugin, conflicted.id, scope(), Duration::from_secs(60))
        .await
        .unwrap();
    vault.bump_revision(conflicted.id).await;
    assert_eq!(
        conflict_service
            .commit_refresh(
                &plugin,
                &conflict_session,
                ValidatedCredentialUpdate::active(origins(&[ORIGIN])),
            )
            .await,
        Err(CredentialProxyError::RefreshConflict)
    );
}

#[tokio::test]
async fn rejects_refresh_seed_values_that_cannot_be_emitted_as_one_cookie_value() {
    let vault = TestVault::new();
    let record = vault.create_cookie_value("unsafe;truncated").await;
    let service = service(ScriptedTransport::default(), vault, TestGrants::new(None));

    assert_eq!(
        service
            .create_refresh_jar(
                &credential_plugin(ORIGIN),
                record.id,
                scope(),
                Duration::from_secs(60),
            )
            .await,
        Err(CredentialProxyError::SecretUnavailable)
    );
}

#[tokio::test]
async fn refresh_seed_accepts_cookie_parser_quoted_values_without_rewriting_them() {
    let vault = TestVault::new();
    let record = vault.create_cookie_value("\"two words\"").await;
    let transport = ScriptedTransport::with_responses([ok_response(Vec::new())]);
    let service = service(transport.clone(), vault, TestGrants::new(None));
    let plugin = credential_plugin(ORIGIN);
    let session = service
        .create_refresh_jar(&plugin, record.id, scope(), Duration::from_secs(60))
        .await
        .expect("quoted refresh Cookie");

    service
        .execute(&plugin, jar_request(session, "/quoted"))
        .await
        .unwrap();
    assert_eq!(
        transport.seen()[0].headers[header::COOKIE],
        "session=\"two words\""
    );
}

#[tokio::test]
async fn rejects_stale_jar_manifest_provided_only_content_scope_and_revoked_credentials() {
    let vault = TestVault::new();
    let record = vault.create_cookie(None).await;
    let transport = ScriptedTransport::default();
    let grants = TestGrants::new(Some(grant_for(&record.metadata)));
    let service = service(transport.clone(), vault.clone(), grants.clone());
    let plugin = credential_plugin(ORIGIN);
    let session = service
        .create_login_jar(
            &plugin,
            scope(),
            origins(&[ORIGIN]),
            Duration::from_secs(60),
        )
        .unwrap();
    let mut reinstalled = plugin.clone();
    reinstalled.manifest_hash = "d".repeat(64);
    assert_eq!(
        service
            .execute(&reinstalled, jar_request(session, "/stale"))
            .await,
        Err(CredentialProxyError::JarBindingMismatch)
    );

    let provided_only = provided_only_content_plugin();
    assert_eq!(
        service
            .execute(
                &provided_only,
                credential_request(record.id, "/provided-only"),
            )
            .await,
        Err(CredentialProxyError::ScopeNotDeclared)
    );

    vault.revoke(record.id).await;
    assert_eq!(
        service
            .execute(
                &content_plugin(MANIFEST_HASH, ORIGIN),
                credential_request(record.id, "/revoked"),
            )
            .await,
        Err(CredentialProxyError::CredentialRevoked)
    );
    assert!(transport.seen().is_empty());
}

#[tokio::test]
async fn structured_proxy_logs_exclude_secrets_urls_headers_and_bodies() {
    let vault = TestVault::new();
    let record = vault.create_cookie(None).await;
    let transport = ScriptedTransport::with_responses([ok_response(b"safe".to_vec())]);
    let service = service(
        transport,
        vault,
        TestGrants::new(Some(grant_for(&record.metadata))),
    );
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let mut request = credential_request(
        record.id,
        "/private-path-canary?query=query-canary-must-not-log",
    );
    request.request.body = b"body-canary-must-not-log".to_vec();
    request
        .request
        .headers
        .insert(header::ACCEPT, HeaderValue::from_static("audio/test"));

    service
        .execute(&content_plugin(MANIFEST_HASH, ORIGIN), request)
        .with_subscriber(subscriber)
        .await
        .unwrap();

    let rendered = logs.text();
    assert!(rendered.contains("credential_proxy"));
    assert!(rendered.contains("plugin_id"));
    for canary in [
        COOKIE_CANARY,
        "private-path-canary",
        "query-canary-must-not-log",
        "body-canary-must-not-log",
        ORIGIN,
        "audio/test",
    ] {
        assert!(!rendered.contains(canary), "log leaked {canary}");
    }
}

#[tokio::test]
async fn rejects_secret_reflection_in_redirect_location_before_following_it() {
    let vault = TestVault::new();
    let record = vault.create_token().await;
    let transport = ScriptedTransport::with_responses([redirect_response(&format!(
        "/next?token={TOKEN_CANARY}"
    ))]);
    let service = service(
        transport.clone(),
        vault,
        TestGrants::new(Some(grant_for(&record.metadata))),
    );

    assert_eq!(
        service
            .execute(
                &content_plugin(MANIFEST_HASH, ORIGIN),
                credential_request(record.id, "/redirect-reflection"),
            )
            .await,
        Err(CredentialProxyError::DirectReflection)
    );
    assert_eq!(transport.seen().len(), 1);
}

#[derive(Clone, Default)]
struct ScriptedTransport {
    responses: Arc<Mutex<VecDeque<TransportResponse>>>,
    seen: Arc<Mutex<Vec<TransportRequest>>>,
}

impl ScriptedTransport {
    fn with_responses(responses: impl IntoIterator<Item = TransportResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            seen: Arc::default(),
        }
    }

    fn seen(&self) -> Vec<TransportRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl HttpTransport for ScriptedTransport {
    async fn execute(
        &self,
        _target: &audiodown_network_proxy::policy::PinnedTarget,
        request: TransportRequest,
    ) -> Result<TransportResponse, TransportError> {
        self.seen.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(TransportError)
    }
}

#[derive(Clone)]
struct TestVault {
    vault: CredentialVault<MemoryRepository>,
    metadata_overrides: Arc<Mutex<HashMap<CredentialId, CredentialKind>>>,
}

impl TestVault {
    fn new() -> Self {
        Self {
            vault: CredentialVault::new(
                MasterKey::from_secret(Secret::new([0xA5; 32])),
                MemoryRepository::default(),
            ),
            metadata_overrides: Arc::default(),
        }
    }

    async fn create_cookie(&self, expires_at: Option<chrono::DateTime<Utc>>) -> StoredCredential {
        self.create_cookie_value_with_expiry(COOKIE_CANARY, expires_at)
            .await
    }

    async fn create_cookie_value(&self, value: &str) -> StoredCredential {
        self.create_cookie_value_with_expiry(value, None).await
    }

    async fn create_cookie_value_with_expiry(
        &self,
        value: &str,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> StoredCredential {
        self.vault
            .trusted()
            .create_cookie(
                create_request(expires_at),
                CookieCredentialSecret::new(vec![CookieSecretRecord::new(
                    "session",
                    SecretString::new(value.to_string()),
                    HOST,
                    "/",
                    false,
                    true,
                    None,
                )
                .unwrap()])
                .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn create_token(&self) -> StoredCredential {
        self.vault
            .trusted()
            .create_token(
                create_request(None),
                TokenCredentialSecret::bearer(SecretString::new(TOKEN_CANARY.to_string())).unwrap(),
            )
            .await
            .unwrap()
    }

    fn force_metadata_kind(&self, id: CredentialId, kind: CredentialKind) {
        self.metadata_overrides.lock().unwrap().insert(id, kind);
    }

    async fn open_secret(&self, id: CredentialId) -> CredentialSecretGuard {
        self.vault.secrets().open(&id).await.unwrap()
    }

    async fn bump_revision(&self, id: CredentialId) {
        let metadata = self.vault.metadata().get(&id).await.unwrap().unwrap();
        let guard = self.vault.secrets().open(&id).await.unwrap();
        let cookie = guard.cookie().unwrap();
        let secret = CookieCredentialSecret::new(
            cookie
                .cookies()
                .iter()
                .map(|record| {
                    CookieSecretRecord::new(
                        record.name(),
                        SecretString::new(record.with_value(str::to_owned)),
                        record.host(),
                        record.path(),
                        record.secure(),
                        record.http_only(),
                        record.expires_at(),
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        self.vault
            .trusted()
            .update_cookie(update_request(&metadata, metadata.revision), secret)
            .await
            .unwrap();
    }

    async fn revoke(&self, id: CredentialId) {
        let metadata = self.vault.metadata().get(&id).await.unwrap().unwrap();
        let guard = self.vault.secrets().open(&id).await.unwrap();
        let secret = CookieCredentialSecret::new(
            guard
                .cookie()
                .unwrap()
                .cookies()
                .iter()
                .map(|record| {
                    CookieSecretRecord::new(
                        record.name(),
                        SecretString::new(record.with_value(str::to_owned)),
                        record.host(),
                        record.path(),
                        record.secure(),
                        record.http_only(),
                        record.expires_at(),
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        let mut request = update_request(&metadata, metadata.revision);
        request.status = CredentialStatus::Revoked;
        self.vault
            .trusted()
            .update_cookie(request, secret)
            .await
            .unwrap();
    }
}

#[async_trait]
impl CredentialVaultPort for TestVault {
    async fn open_current(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<OpenedCredential>, CredentialPortError> {
        let Some(mut metadata) = self
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
        let current = self
            .vault
            .metadata()
            .get(credential_id)
            .await
            .map_err(map_vault_error)?
            .ok_or(CredentialPortError::NotFound)?;
        if current != metadata {
            return Err(CredentialPortError::Conflict);
        }
        if let Some(kind) = self.metadata_overrides.lock().unwrap().get(credential_id) {
            metadata.kind = *kind;
        }
        Ok(Some(OpenedCredential { metadata, secret }))
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

fn map_vault_error(error: audiodown_credential_vault::VaultError) -> CredentialPortError {
    use audiodown_credential_vault::VaultError;
    match error {
        VaultError::NotFound => CredentialPortError::NotFound,
        VaultError::Conflict => CredentialPortError::Conflict,
        VaultError::Expired => CredentialPortError::Expired,
        VaultError::Revoked => CredentialPortError::Revoked,
        _ => CredentialPortError::Unavailable,
    }
}

#[derive(Clone)]
struct TestGrants {
    grant: Arc<Mutex<Option<ActiveGrantSnapshot>>>,
}

impl TestGrants {
    fn new(grant: Option<ActiveGrantSnapshot>) -> Self {
        Self {
            grant: Arc::new(Mutex::new(grant)),
        }
    }
}

#[async_trait]
impl CredentialGrantPort for TestGrants {
    async fn active_grant(
        &self,
        _plugin_id: &PluginId,
        _credential_id: &CredentialId,
        _scope: &CredentialScope,
    ) -> Result<Option<ActiveGrantSnapshot>, CredentialPortError> {
        Ok(self.grant.lock().unwrap().clone())
    }
}

#[derive(Clone, Default)]
struct MemoryRepository {
    records: Arc<Mutex<HashMap<CredentialId, StoredCredential>>>,
}

#[async_trait]
impl CredentialRepository for MemoryRepository {
    async fn insert(&self, record: &StoredCredential) -> Result<(), CredentialRepositoryError> {
        let mut records = self.records.lock().unwrap();
        if records.contains_key(&record.id) {
            return Err(CredentialRepositoryError::Conflict);
        }
        records.insert(record.id, record.clone());
        Ok(())
    }

    async fn update(
        &self,
        record: &StoredCredential,
        expected_revision: u64,
    ) -> Result<u64, CredentialRepositoryError> {
        let mut records = self.records.lock().unwrap();
        let existing = records
            .get(&record.id)
            .ok_or(CredentialRepositoryError::NotFound)?;
        if existing.revision != expected_revision {
            return Err(CredentialRepositoryError::Conflict);
        }
        let revision = expected_revision + 1;
        let mut stored = record.clone();
        stored.revision = revision;
        stored.metadata.revision = revision;
        records.insert(record.id, stored);
        Ok(revision)
    }

    async fn get(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<StoredCredential>, CredentialRepositoryError> {
        Ok(self.records.lock().unwrap().get(credential_id).cloned())
    }

    async fn list(&self) -> Result<Vec<StoredCredential>, CredentialRepositoryError> {
        Ok(self.records.lock().unwrap().values().cloned().collect())
    }

    async fn delete(&self, credential_id: &CredentialId) -> Result<(), CredentialRepositoryError> {
        self.records
            .lock()
            .unwrap()
            .remove(credential_id)
            .map(|_| ())
            .ok_or(CredentialRepositoryError::NotFound)
    }

    async fn clear_source_plugin(
        &self,
        credential_id: &CredentialId,
    ) -> Result<(), CredentialRepositoryError> {
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(credential_id)
            .ok_or(CredentialRepositoryError::NotFound)?;
        record.source_plugin_id = None;
        record.metadata.ownership = audiodown_domain::credential::CredentialOwnership::Retained;
        Ok(())
    }
}

fn service<T: HttpTransport>(
    transport: T,
    vault: TestVault,
    grants: TestGrants,
) -> CredentialProxyService<StaticResolver, T, TestVault, TestGrants> {
    let manifest = content_manifest(MANIFEST_HASH, ORIGIN);
    let policy = ProxyPolicy::developer(&manifest)
        .with_fixture_mapping(HOST, IpAddr::V4(Ipv4Addr::LOCALHOST))
        .unwrap();
    CredentialProxyService::new(
        HttpProxy::new(policy, StaticResolver::empty(), transport),
        vault,
        grants,
    )
}

fn content_plugin(hash: &str, declared_origin: &str) -> InstalledPluginContext {
    let manifest = content_manifest(hash, declared_origin);
    InstalledPluginContext {
        plugin_id: manifest.id.clone(),
        manifest_hash: hash.to_string(),
        manifest,
    }
}

fn credential_plugin(declared_origin: &str) -> InstalledPluginContext {
    let manifest: PluginManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.example.virtual.credential",
        "name": "Virtual Credential",
        "version": "1.0.0",
        "type": "credential",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": ["credential.status"],
        "network": {"allowedHosts": [HOST]},
        "credentials": {
            "providedScopes": [{
                "scope": "virtual.web",
                "targetOrigins": [declared_origin]
            }]
        }
    }))
    .unwrap();
    InstalledPluginContext {
        plugin_id: manifest.id.clone(),
        manifest_hash: "c".repeat(64),
        manifest,
    }
}

fn provided_only_content_plugin() -> InstalledPluginContext {
    let mut value = serde_json::to_value(content_manifest(MANIFEST_HASH, ORIGIN)).unwrap();
    let credentials = value
        .get_mut("credentials")
        .and_then(serde_json::Value::as_object_mut)
        .unwrap();
    let declarations = credentials.remove("requiredScopes").unwrap();
    credentials.insert("providedScopes".to_string(), declarations);
    let manifest: PluginManifest = serde_json::from_value(value).unwrap();
    InstalledPluginContext {
        plugin_id: manifest.id.clone(),
        manifest_hash: MANIFEST_HASH.to_string(),
        manifest,
    }
}

fn content_manifest(_hash: &str, declared_origin: &str) -> PluginManifest {
    serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.example.virtual.content",
        "name": "Virtual Content",
        "version": "1.0.0",
        "type": "content",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": ["system.health"],
        "network": {"allowedHosts": [HOST]},
        "credentials": {
            "requiredScopes": [{
                "scope": "virtual.web",
                "targetOrigins": [declared_origin]
            }]
        }
    }))
    .unwrap()
}

fn create_request(expires_at: Option<chrono::DateTime<Utc>>) -> CredentialCreateRequest {
    CredentialCreateRequest {
        platform_id: "virtual".to_string(),
        scope: scope(),
        source_plugin_id: Some(PluginId::parse("com.example.virtual.credential").unwrap()),
        target_origins: origins(&[ORIGIN]),
        account_id_hint: None,
        display_name: None,
        expires_at,
    }
}

fn update_request(
    metadata: &CredentialMetadata,
    expected_revision: u64,
) -> CredentialUpdateRequest {
    CredentialUpdateRequest {
        credential_id: metadata.id,
        expected_revision,
        target_origins: metadata.target_origins.clone(),
        account_id_hint: metadata.account_id_hint.clone(),
        display_name: metadata.display_name.clone(),
        status: CredentialStatus::Active,
        safe_error_summary: None,
        expires_at: metadata.expires_at,
        status_checked_at: Some(Utc::now()),
    }
}

fn grant_for(metadata: &CredentialMetadata) -> ActiveGrantSnapshot {
    ActiveGrantSnapshot {
        plugin_id: PluginId::parse("com.example.virtual.content").unwrap(),
        manifest_hash: MANIFEST_HASH.to_string(),
        credential_id: metadata.id,
        scope: metadata.scope.clone(),
        target_origins: metadata.target_origins.clone(),
    }
}

fn credential_request(id: CredentialId, path: &str) -> CredentialProxyRequest {
    CredentialProxyRequest {
        request: request(path),
        cookie_jar_session_id: None,
        credential: Some(CredentialSelection {
            credential_id: id,
            scope: scope(),
        }),
    }
}

fn jar_request(
    id: audiodown_network_proxy::cookie_jar::CookieJarSessionId,
    path: &str,
) -> CredentialProxyRequest {
    CredentialProxyRequest {
        request: request(path),
        cookie_jar_session_id: Some(id),
        credential: None,
    }
}

fn request(path: &str) -> ProxyRequest {
    ProxyRequest {
        method: Method::GET,
        url: format!("{ORIGIN}{path}"),
        headers: HeaderMap::new(),
        body: Vec::new(),
    }
}

fn scope() -> CredentialScope {
    CredentialScope::parse("virtual.web").unwrap()
}

fn origins(values: &[&str]) -> Vec<CredentialTargetOrigin> {
    values
        .iter()
        .map(|value| CredentialTargetOrigin::parse(value).unwrap())
        .collect()
}

fn ok_response(body: Vec<u8>) -> TransportResponse {
    TransportResponse {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        body,
    }
}

fn response_with_set_cookie(cookie: &str, body: Vec<u8>) -> TransportResponse {
    let mut response = ok_response(body);
    response
        .headers
        .append(header::SET_COOKIE, HeaderValue::from_str(cookie).unwrap());
    response
}

fn redirect_response(location: &str) -> TransportResponse {
    let mut response = ok_response(Vec::new());
    response.status = StatusCode::FOUND;
    response
        .headers
        .insert(header::LOCATION, HeaderValue::from_str(location).unwrap());
    response
}

fn redirect_with_cookie(location: &str, cookie: &str) -> TransportResponse {
    let mut response = redirect_response(location);
    response
        .headers
        .append(header::SET_COOKIE, HeaderValue::from_str(cookie).unwrap());
    response
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl LogBuffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}
