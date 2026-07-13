use std::{fmt, sync::Mutex, time::Duration};

use async_trait::async_trait;
use audiodown_credential_vault::{
    CookieCredentialSecret, CredentialMetadata, CredentialUpdateRequest,
};
use audiodown_domain::credential::{CredentialKind, CredentialScope, CredentialStatus};
use audiodown_plugin_api::manifest::{
    CredentialScopeDeclaration, CredentialTargetOrigin, PluginType,
};
use chrono::{DateTime, Utc};
use http::{header, HeaderValue};
use thiserror::Error;
use url::Url;
use zeroize::Zeroizing;

use crate::{
    cookie_jar::{
        CookieJarBinding, CookieJarError, CookieJarSessionId, PromotionSnapshot,
        TemporaryCookieJars,
    },
    credential::{
        ActiveGrantSnapshot, CredentialGrantPort, CredentialPortError, CredentialSelection,
        CredentialVaultPort, InstalledPluginContext, OpenedCredential,
    },
    error::HttpProxyError,
    http::{
        HttpHookError, HttpHopHook, HttpProxy, HttpTransport, ProxyRequest, ProxyResponse,
        TransportRequest, TransportResponse,
    },
    policy::PinnedTarget,
    resolver::DnsResolver,
};

const MAX_AUTH_HEADER_BYTES: usize = 16 * 1024;
const MAX_COOKIE_COUNT: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialProxyRequest {
    pub request: ProxyRequest,
    pub cookie_jar_session_id: Option<CookieJarSessionId>,
    pub credential: Option<CredentialSelection>,
}

#[derive(Debug, Clone)]
pub struct ValidatedCredentialUpdate {
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub account_id_hint: Option<String>,
    pub display_name: Option<String>,
    pub status: CredentialStatus,
    pub safe_error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status_checked_at: Option<DateTime<Utc>>,
}

impl ValidatedCredentialUpdate {
    pub fn active(target_origins: Vec<CredentialTargetOrigin>) -> Self {
        Self {
            target_origins,
            account_id_hint: None,
            display_name: None,
            status: CredentialStatus::Active,
            safe_error_summary: None,
            expires_at: None,
            status_checked_at: Some(Utc::now()),
        }
    }
}

pub struct CredentialProxyService<R, T, V, G> {
    http: HttpProxy<R, T>,
    jars: TemporaryCookieJars,
    vault: V,
    grants: G,
}

impl<R, T, V, G> CredentialProxyService<R, T, V, G>
where
    R: DnsResolver + Send + 'static,
    T: HttpTransport,
    V: CredentialVaultPort,
    G: CredentialGrantPort,
{
    pub fn new(http: HttpProxy<R, T>, vault: V, grants: G) -> Self {
        Self {
            http,
            jars: TemporaryCookieJars::new(),
            vault,
            grants,
        }
    }

    pub fn create_login_jar(
        &self,
        plugin: &InstalledPluginContext,
        scope: CredentialScope,
        target_origins: Vec<CredentialTargetOrigin>,
        ttl: Duration,
    ) -> Result<CookieJarSessionId, CredentialProxyError> {
        audit(plugin, "login_jar_create", "started");
        let declaration = provider_declaration(plugin, &scope)?;
        let target_origins = declared_origins(&target_origins, declaration)?;
        let binding = CookieJarBinding::login(
            plugin.plugin_id.clone(),
            plugin.manifest.platform.id.clone(),
            scope,
            target_origins,
        )
        .map_err(map_jar_error)?
        .with_manifest_hash(plugin.manifest_hash.clone());
        let result = self.jars.create(binding, ttl).map_err(map_jar_error);
        audit(plugin, "login_jar_create", result_code(&result));
        result
    }

    pub async fn create_refresh_jar(
        &self,
        plugin: &InstalledPluginContext,
        credential_id: audiodown_domain::credential::CredentialId,
        scope: CredentialScope,
        ttl: Duration,
    ) -> Result<CookieJarSessionId, CredentialProxyError> {
        audit(plugin, "refresh_jar_create", "started");
        let declaration = provider_declaration(plugin, &scope)?;
        let opened = self
            .vault
            .open_current(&credential_id)
            .await
            .map_err(map_port_error)?
            .ok_or(CredentialProxyError::CredentialNotFound)?;
        validate_refresh_credential(plugin, &scope, declaration, &opened)?;
        let cookie = opened
            .secret
            .cookie()
            .ok_or(CredentialProxyError::SecretUnavailable)?;
        let binding = CookieJarBinding::refresh(
            plugin.plugin_id.clone(),
            plugin.manifest.platform.id.clone(),
            scope,
            opened.metadata.target_origins.clone(),
            credential_id,
            opened.metadata.revision,
        )
        .map_err(map_jar_error)?
        .with_manifest_hash(plugin.manifest_hash.clone());
        let result = self
            .jars
            .create_seeded(binding, ttl, cookie)
            .map_err(map_jar_error);
        audit(plugin, "refresh_jar_create", result_code(&result));
        result
    }

    pub async fn execute(
        &self,
        plugin: &InstalledPluginContext,
        request: CredentialProxyRequest,
    ) -> Result<ProxyResponse, CredentialProxyError> {
        audit(plugin, "credential_proxy", "started");
        validate_plugin_context(plugin)?;
        if request.cookie_jar_session_id.is_some() && request.credential.is_some() {
            return Err(CredentialProxyError::InvalidRequest);
        }
        if request.cookie_jar_session_id.is_none() && request.credential.is_none() {
            let result = self
                .http
                .execute(request.request)
                .await
                .map_err(CredentialProxyError::Proxy);
            audit(plugin, "http_proxy", result_code(&result));
            return result;
        }

        let mode = if let Some(session_id) = request.cookie_jar_session_id {
            let binding = self.jars.binding(&session_id).map_err(map_jar_error)?;
            validate_jar_binding(plugin, &binding)?;
            HookMode::Jar {
                session_id,
                binding,
            }
        } else {
            HookMode::Credential(
                request
                    .credential
                    .ok_or(CredentialProxyError::InvalidRequest)?,
            )
        };
        let hook = CredentialHttpHook {
            mode,
            plugin,
            jars: &self.jars,
            vault: &self.vault,
            grants: &self.grants,
            rejection: Mutex::new(None),
        };
        let result = match self.http.execute_with_hook(request.request, &hook).await {
            Ok(response) => Ok(response),
            Err(HttpProxyError::RequestRejected) => Err(hook
                .take_rejection()
                .unwrap_or(CredentialProxyError::RequestRejected)),
            Err(error) => Err(CredentialProxyError::Proxy(error)),
        };
        audit(plugin, "credential_proxy", result_code(&result));
        result
    }

    pub fn promotion_snapshot(
        &self,
        plugin: &InstalledPluginContext,
        session_id: &CookieJarSessionId,
        target_origins: &[CredentialTargetOrigin],
    ) -> Result<PromotionSnapshot, CredentialProxyError> {
        audit(plugin, "login_jar_snapshot", "started");
        let binding = self.jars.binding(session_id).map_err(map_jar_error)?;
        validate_jar_binding(plugin, &binding)?;
        let result = self
            .jars
            .promotion_snapshot(session_id, &binding, target_origins)
            .map_err(map_jar_error);
        audit(plugin, "login_jar_snapshot", result_code(&result));
        result
    }

    pub async fn commit_refresh(
        &self,
        plugin: &InstalledPluginContext,
        session_id: &CookieJarSessionId,
        update: ValidatedCredentialUpdate,
    ) -> Result<CredentialMetadata, CredentialProxyError> {
        audit(plugin, "refresh_commit", "started");
        let binding = self.jars.binding(session_id).map_err(map_jar_error)?;
        validate_jar_binding(plugin, &binding)?;
        provider_declaration(plugin, binding.scope())?;
        let snapshot = self
            .jars
            .begin_refresh_commit(session_id, &binding, &update.target_origins)
            .map_err(map_jar_error)?;
        let request = CredentialUpdateRequest {
            credential_id: snapshot.credential_id,
            expected_revision: snapshot.expected_revision,
            target_origins: snapshot.target_origins,
            account_id_hint: update.account_id_hint,
            display_name: update.display_name,
            status: update.status,
            safe_error_summary: update.safe_error_summary,
            expires_at: update.expires_at,
            status_checked_at: update.status_checked_at,
        };
        let result = match self.vault.replace_cookie(request, snapshot.secret).await {
            Ok(metadata) => {
                self.jars
                    .finish_refresh(session_id, false)
                    .map_err(map_jar_error)?;
                Ok(metadata)
            }
            Err(CredentialPortError::Conflict) => {
                let _ = self.jars.finish_refresh(session_id, false);
                Err(CredentialProxyError::RefreshConflict)
            }
            Err(CredentialPortError::Unavailable) => {
                let _ = self.jars.finish_refresh(session_id, true);
                Err(CredentialProxyError::Unavailable)
            }
            Err(error) => {
                let _ = self.jars.finish_refresh(session_id, false);
                Err(map_port_error(error))
            }
        };
        audit(plugin, "refresh_commit", result_code(&result));
        result
    }

    pub fn cancel_jar(
        &self,
        plugin: &InstalledPluginContext,
        session_id: &CookieJarSessionId,
    ) -> Result<(), CredentialProxyError> {
        audit(plugin, "cookie_jar_cancel", "started");
        let binding = self.jars.binding(session_id).map_err(map_jar_error)?;
        validate_jar_binding(plugin, &binding)?;
        let result = self
            .jars
            .cancel(session_id, &binding)
            .map_err(map_jar_error);
        audit(plugin, "cookie_jar_cancel", result_code(&result));
        result
    }
}

enum HookMode {
    Jar {
        session_id: CookieJarSessionId,
        binding: CookieJarBinding,
    },
    Credential(CredentialSelection),
}

struct CredentialHttpHook<'a, V, G> {
    mode: HookMode,
    plugin: &'a InstalledPluginContext,
    jars: &'a TemporaryCookieJars,
    vault: &'a V,
    grants: &'a G,
    rejection: Mutex<Option<CredentialProxyError>>,
}

impl<V, G> CredentialHttpHook<'_, V, G> {
    fn reject<T>(&self, error: CredentialProxyError) -> Result<T, HttpHookError> {
        if let Ok(mut rejection) = self.rejection.lock() {
            *rejection = Some(error);
        }
        Err(HttpHookError)
    }

    fn take_rejection(&self) -> Option<CredentialProxyError> {
        self.rejection
            .lock()
            .ok()
            .and_then(|mut rejection| rejection.take())
    }
}

#[async_trait]
impl<V, G> HttpHopHook for CredentialHttpHook<'_, V, G>
where
    V: CredentialVaultPort,
    G: CredentialGrantPort,
{
    type HopState = HopSecretState;

    async fn prepare(
        &self,
        target: &PinnedTarget,
        request: &mut TransportRequest,
    ) -> Result<Self::HopState, HttpHookError> {
        let result = match &self.mode {
            HookMode::Jar {
                session_id,
                binding,
            } => prepare_jar_hop(self.plugin, self.jars, session_id, binding, target, request),
            HookMode::Credential(selection) => {
                prepare_credential_hop(
                    self.plugin,
                    self.vault,
                    self.grants,
                    selection,
                    target,
                    request,
                )
                .await
            }
        };
        match result {
            Ok(state) => Ok(state),
            Err(error) => self.reject(error),
        }
    }

    async fn observe(
        &self,
        target: &PinnedTarget,
        state: &Self::HopState,
        response: &TransportResponse,
    ) -> Result<(), HttpHookError> {
        for (name, value) in &response.headers {
            if name != header::SET_COOKIE && state.contains(value.as_bytes()) {
                return self.reject(CredentialProxyError::DirectReflection);
            }
        }
        if let HookMode::Jar {
            session_id,
            binding,
        } = &self.mode
        {
            if let Err(error) =
                self.jars
                    .capture_response(session_id, binding, target.url(), &response.headers)
            {
                return self.reject(map_jar_error(error));
            }
        }
        Ok(())
    }

    fn validate_visible(
        &self,
        _target: &PinnedTarget,
        state: &Self::HopState,
        response: &ProxyResponse,
    ) -> Result<(), HttpHookError> {
        if response
            .headers
            .values()
            .any(|value| state.contains(value.as_bytes()))
            || state.contains(&response.body)
        {
            self.reject(CredentialProxyError::DirectReflection)
        } else {
            Ok(())
        }
    }
}

struct HopSecretState {
    markers: Vec<Zeroizing<Vec<u8>>>,
}

impl HopSecretState {
    fn empty() -> Self {
        Self { markers: vec![] }
    }

    fn from_cookie_header(value: &HeaderValue) -> Result<Self, CredentialProxyError> {
        let value = value
            .to_str()
            .map_err(|_| CredentialProxyError::SecretUnavailable)?;
        let mut markers = vec![value.as_bytes().to_vec()];
        for pair in value.split("; ") {
            markers.push(pair.as_bytes().to_vec());
            if let Some((_, secret)) = pair.split_once('=') {
                markers.push(secret.as_bytes().to_vec());
            }
        }
        Ok(Self::new(markers))
    }

    fn from_token(token: &str) -> Self {
        Self::new(vec![
            token.as_bytes().to_vec(),
            format!("Bearer {token}").into_bytes(),
        ])
    }

    fn new(mut markers: Vec<Vec<u8>>) -> Self {
        markers.retain(|marker| !marker.is_empty());
        markers.sort();
        markers.dedup();
        Self {
            markers: markers.into_iter().map(Zeroizing::new).collect(),
        }
    }

    fn contains(&self, haystack: &[u8]) -> bool {
        self.markers
            .iter()
            .any(|marker| contains_subslice(haystack, marker))
    }
}

impl fmt::Debug for HopSecretState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HopSecretState")
            .field("markers", &"[REDACTED]")
            .finish()
    }
}

fn prepare_jar_hop(
    plugin: &InstalledPluginContext,
    jars: &TemporaryCookieJars,
    session_id: &CookieJarSessionId,
    binding: &CookieJarBinding,
    target: &PinnedTarget,
    request: &mut TransportRequest,
) -> Result<HopSecretState, CredentialProxyError> {
    validate_jar_binding(plugin, binding)?;
    let declaration = provider_declaration(plugin, binding.scope())?;
    let origin = request_origin(target.url())?;
    if !binding.target_origins().contains(&origin) || !declaration.target_origins.contains(&origin)
    {
        return Err(CredentialProxyError::OriginDenied);
    }
    let Some(cookie) = jars
        .cookie_header(session_id, binding, target.url())
        .map_err(map_jar_error)?
    else {
        return Ok(HopSecretState::empty());
    };
    let state = HopSecretState::from_cookie_header(&cookie)?;
    request.headers.insert(header::COOKIE, cookie);
    Ok(state)
}

async fn prepare_credential_hop<V, G>(
    plugin: &InstalledPluginContext,
    vault: &V,
    grants: &G,
    selection: &CredentialSelection,
    target: &PinnedTarget,
    request: &mut TransportRequest,
) -> Result<HopSecretState, CredentialProxyError>
where
    V: CredentialVaultPort,
    G: CredentialGrantPort,
{
    let declaration = consumer_declaration(plugin, &selection.scope)?;
    let opened = vault
        .open_current(&selection.credential_id)
        .await
        .map_err(map_port_error)?
        .ok_or(CredentialProxyError::CredentialNotFound)?;
    let grant = grants
        .active_grant(
            &plugin.plugin_id,
            &selection.credential_id,
            &selection.scope,
        )
        .await
        .map_err(map_port_error)?
        .ok_or(CredentialProxyError::GrantMissing)?;
    let origin = request_origin(target.url())?;
    authorize_credential(plugin, selection, declaration, &opened, &grant, &origin)?;

    match opened.metadata.kind {
        CredentialKind::Cookie => {
            let secret = opened
                .secret
                .cookie()
                .ok_or(CredentialProxyError::SecretUnavailable)?;
            let cookie = stored_cookie_header(secret, target.url())?
                .ok_or(CredentialProxyError::SecretUnavailable)?;
            let state = HopSecretState::from_cookie_header(&cookie)?;
            request.headers.insert(header::COOKIE, cookie);
            Ok(state)
        }
        CredentialKind::Token => {
            let token = opened
                .secret
                .token()
                .ok_or(CredentialProxyError::SecretUnavailable)?;
            token.with_value(|token| {
                let encoded = Zeroizing::new(format!("Bearer {token}"));
                let value = HeaderValue::from_str(&encoded)
                    .map_err(|_| CredentialProxyError::SecretUnavailable)?;
                if value.as_bytes().len() > MAX_AUTH_HEADER_BYTES {
                    return Err(CredentialProxyError::SecretUnavailable);
                }
                let state = HopSecretState::from_token(token);
                request.headers.insert(header::AUTHORIZATION, value);
                Ok(state)
            })
        }
    }
}

fn authorize_credential(
    plugin: &InstalledPluginContext,
    selection: &CredentialSelection,
    declaration: &CredentialScopeDeclaration,
    opened: &OpenedCredential,
    grant: &ActiveGrantSnapshot,
    origin: &CredentialTargetOrigin,
) -> Result<(), CredentialProxyError> {
    let metadata = &opened.metadata;
    if metadata.id != selection.credential_id || metadata.scope != selection.scope {
        return Err(CredentialProxyError::CredentialMismatch);
    }
    if metadata.platform_id != plugin.manifest.platform.id {
        return Err(CredentialProxyError::CredentialMismatch);
    }
    if metadata.status == CredentialStatus::Revoked {
        return Err(CredentialProxyError::CredentialRevoked);
    }
    if metadata.status != CredentialStatus::Active
        || metadata
            .expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now())
    {
        return Err(CredentialProxyError::CredentialExpired);
    }
    if grant.plugin_id != plugin.plugin_id
        || grant.manifest_hash != plugin.manifest_hash
        || grant.credential_id != selection.credential_id
        || grant.scope != selection.scope
    {
        return Err(CredentialProxyError::GrantMismatch);
    }
    if grant.target_origins.is_empty()
        || !declaration.target_origins.contains(origin)
        || !metadata.target_origins.contains(origin)
        || !grant.target_origins.contains(origin)
        || grant.target_origins.iter().any(|granted| {
            !declaration.target_origins.contains(granted)
                || !metadata.target_origins.contains(granted)
        })
    {
        return Err(CredentialProxyError::OriginDenied);
    }
    Ok(())
}

fn stored_cookie_header(
    secret: &CookieCredentialSecret,
    url: &Url,
) -> Result<Option<HeaderValue>, CredentialProxyError> {
    let host = url.host_str().ok_or(CredentialProxyError::OriginDenied)?;
    let now = Utc::now();
    let mut cookies = secret
        .cookies()
        .iter()
        .filter(|cookie| {
            cookie.host().eq_ignore_ascii_case(host)
                && (!cookie.secure() || url.scheme() == "https")
                && cookie
                    .expires_at()
                    .is_none_or(|expires_at| expires_at > now)
                && path_matches(cookie.path(), url.path())
        })
        .collect::<Vec<_>>();
    cookies.sort_by(|left, right| {
        right
            .path()
            .len()
            .cmp(&left.path().len())
            .then_with(|| left.name().cmp(right.name()))
    });
    if cookies.len() > MAX_COOKIE_COUNT {
        return Err(CredentialProxyError::SecretUnavailable);
    }
    let mut value = Zeroizing::new(String::new());
    for cookie in cookies {
        if !value.is_empty() {
            value.push_str("; ");
        }
        value.push_str(cookie.name());
        value.push('=');
        cookie.with_value(|cookie_value| value.push_str(cookie_value));
        if value.len() > MAX_AUTH_HEADER_BYTES {
            return Err(CredentialProxyError::SecretUnavailable);
        }
    }
    if value.is_empty() {
        Ok(None)
    } else {
        HeaderValue::from_str(&value)
            .map(Some)
            .map_err(|_| CredentialProxyError::SecretUnavailable)
    }
}

fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    cookie_path == request_path
        || (request_path.starts_with(cookie_path)
            && (cookie_path.ends_with('/')
                || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')))
}

fn validate_refresh_credential(
    plugin: &InstalledPluginContext,
    scope: &CredentialScope,
    declaration: &CredentialScopeDeclaration,
    opened: &OpenedCredential,
) -> Result<(), CredentialProxyError> {
    let metadata = &opened.metadata;
    if metadata.kind != CredentialKind::Cookie
        || metadata.platform_id != plugin.manifest.platform.id
        || &metadata.scope != scope
    {
        return Err(CredentialProxyError::CredentialMismatch);
    }
    if metadata.status == CredentialStatus::Revoked {
        return Err(CredentialProxyError::CredentialRevoked);
    }
    if metadata.status != CredentialStatus::Active
        || metadata
            .expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now())
    {
        return Err(CredentialProxyError::CredentialExpired);
    }
    if metadata.target_origins.is_empty()
        || metadata
            .target_origins
            .iter()
            .any(|origin| !declaration.target_origins.contains(origin))
    {
        return Err(CredentialProxyError::OriginDenied);
    }
    Ok(())
}

fn validate_jar_binding(
    plugin: &InstalledPluginContext,
    binding: &CookieJarBinding,
) -> Result<(), CredentialProxyError> {
    validate_plugin_context(plugin)?;
    if plugin.manifest.plugin_type != PluginType::Credential
        || binding.plugin_id() != &plugin.plugin_id
        || binding.platform_id() != plugin.manifest.platform.id
        || binding.manifest_hash() != Some(plugin.manifest_hash.as_str())
    {
        return Err(CredentialProxyError::JarBindingMismatch);
    }
    Ok(())
}

fn validate_plugin_context(plugin: &InstalledPluginContext) -> Result<(), CredentialProxyError> {
    if plugin.plugin_id != plugin.manifest.id
        || plugin.manifest_hash.len() != 64
        || !plugin
            .manifest_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Err(CredentialProxyError::InvalidRequest)
    } else {
        Ok(())
    }
}

fn provider_declaration<'a>(
    plugin: &'a InstalledPluginContext,
    scope: &CredentialScope,
) -> Result<&'a CredentialScopeDeclaration, CredentialProxyError> {
    validate_plugin_context(plugin)?;
    if plugin.manifest.plugin_type != PluginType::Credential {
        return Err(CredentialProxyError::ScopeNotDeclared);
    }
    plugin
        .manifest
        .credentials
        .provided_scopes
        .iter()
        .find(|declaration| &declaration.scope == scope)
        .ok_or(CredentialProxyError::ScopeNotDeclared)
}

fn consumer_declaration<'a>(
    plugin: &'a InstalledPluginContext,
    scope: &CredentialScope,
) -> Result<&'a CredentialScopeDeclaration, CredentialProxyError> {
    validate_plugin_context(plugin)?;
    if plugin.manifest.plugin_type != PluginType::Content {
        return Err(CredentialProxyError::ScopeNotDeclared);
    }
    plugin
        .manifest
        .credentials
        .required_scopes
        .iter()
        .chain(plugin.manifest.credentials.optional_scopes.iter())
        .find(|declaration| &declaration.scope == scope)
        .ok_or(CredentialProxyError::ScopeNotDeclared)
}

fn declared_origins(
    requested: &[CredentialTargetOrigin],
    declaration: &CredentialScopeDeclaration,
) -> Result<Vec<CredentialTargetOrigin>, CredentialProxyError> {
    if requested.is_empty() {
        return Err(CredentialProxyError::OriginDenied);
    }
    let mut normalized = requested
        .iter()
        .map(|origin| {
            CredentialTargetOrigin::parse(origin.as_str())
                .map_err(|_| CredentialProxyError::OriginDenied)
        })
        .collect::<Result<Vec<_>, _>>()?;
    normalized.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    normalized.dedup();
    if normalized
        .iter()
        .any(|origin| !declaration.target_origins.contains(origin))
    {
        Err(CredentialProxyError::OriginDenied)
    } else {
        Ok(normalized)
    }
}

fn request_origin(url: &Url) -> Result<CredentialTargetOrigin, CredentialProxyError> {
    CredentialTargetOrigin::parse(url.origin().ascii_serialization())
        .map_err(|_| CredentialProxyError::OriginDenied)
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn map_port_error(error: CredentialPortError) -> CredentialProxyError {
    match error {
        CredentialPortError::NotFound => CredentialProxyError::CredentialNotFound,
        CredentialPortError::Conflict => CredentialProxyError::RefreshConflict,
        CredentialPortError::Expired => CredentialProxyError::CredentialExpired,
        CredentialPortError::Revoked => CredentialProxyError::CredentialRevoked,
        CredentialPortError::Unavailable => CredentialProxyError::Unavailable,
    }
}

fn map_jar_error(error: CookieJarError) -> CredentialProxyError {
    match error {
        CookieJarError::NotFound => CredentialProxyError::JarNotFound,
        CookieJarError::Expired => CredentialProxyError::JarExpired,
        CookieJarError::BindingMismatch => CredentialProxyError::JarBindingMismatch,
        CookieJarError::OriginDenied => CredentialProxyError::OriginDenied,
        CookieJarError::PurposeMismatch => CredentialProxyError::JarPurposeMismatch,
        CookieJarError::Busy => CredentialProxyError::RefreshConflict,
        CookieJarError::InvalidCookie | CookieJarError::HeaderTooLarge => {
            CredentialProxyError::SecretUnavailable
        }
        CookieJarError::InvalidBinding
        | CookieJarError::CapacityReached
        | CookieJarError::Unavailable => CredentialProxyError::Unavailable,
    }
}

fn audit(plugin: &InstalledPluginContext, operation: &'static str, result: &'static str) {
    tracing::info!(
        event = "credential_proxy",
        plugin_id = %plugin.plugin_id,
        operation,
        result
    );
}

fn result_code<T>(result: &Result<T, CredentialProxyError>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(CredentialProxyError::InvalidRequest) => "invalid_request",
        Err(CredentialProxyError::JarNotFound) => "jar_not_found",
        Err(CredentialProxyError::JarExpired) => "jar_expired",
        Err(CredentialProxyError::JarBindingMismatch) => "jar_binding_mismatch",
        Err(CredentialProxyError::JarPurposeMismatch) => "jar_purpose_mismatch",
        Err(CredentialProxyError::CredentialNotFound) => "credential_not_found",
        Err(CredentialProxyError::CredentialExpired) => "credential_expired",
        Err(CredentialProxyError::CredentialRevoked) => "credential_revoked",
        Err(CredentialProxyError::CredentialMismatch) => "credential_mismatch",
        Err(CredentialProxyError::ScopeNotDeclared) => "scope_not_declared",
        Err(CredentialProxyError::GrantMissing) => "grant_missing",
        Err(CredentialProxyError::GrantMismatch) => "grant_mismatch",
        Err(CredentialProxyError::OriginDenied) => "origin_denied",
        Err(CredentialProxyError::SecretUnavailable) => "secret_unavailable",
        Err(CredentialProxyError::RefreshConflict) => "refresh_conflict",
        Err(CredentialProxyError::DirectReflection) => "direct_reflection",
        Err(CredentialProxyError::RequestRejected) => "request_rejected",
        Err(CredentialProxyError::Unavailable) => "unavailable",
        Err(CredentialProxyError::Proxy(_)) => "proxy_error",
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CredentialProxyError {
    #[error("credential proxy request is invalid")]
    InvalidRequest,
    #[error("Cookie Jar was not found")]
    JarNotFound,
    #[error("Cookie Jar has expired")]
    JarExpired,
    #[error("Cookie Jar binding does not match")]
    JarBindingMismatch,
    #[error("Cookie Jar purpose does not match")]
    JarPurposeMismatch,
    #[error("credential was not found")]
    CredentialNotFound,
    #[error("credential has expired")]
    CredentialExpired,
    #[error("credential has been revoked")]
    CredentialRevoked,
    #[error("credential binding does not match")]
    CredentialMismatch,
    #[error("credential scope is not declared")]
    ScopeNotDeclared,
    #[error("credential grant was not found")]
    GrantMissing,
    #[error("credential grant does not match")]
    GrantMismatch,
    #[error("credential target origin is denied")]
    OriginDenied,
    #[error("credential secret is unavailable")]
    SecretUnavailable,
    #[error("credential refresh conflicted")]
    RefreshConflict,
    #[error("credential secret reflection was rejected")]
    DirectReflection,
    #[error("credential request was rejected")]
    RequestRejected,
    #[error("credential proxy is unavailable")]
    Unavailable,
    #[error(transparent)]
    Proxy(HttpProxyError),
}
