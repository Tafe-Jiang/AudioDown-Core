use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use audiodown_credential_vault::{CookieCredentialSecret, CookieSecretRecord, SecretPayloadError};
use audiodown_domain::{
    credential::{CredentialId, CredentialScope},
    plugin::PluginId,
};
use audiodown_plugin_api::manifest::CredentialTargetOrigin;
use chrono::{DateTime, Utc};
use cookie_store::{CookieError, CookieStore, RawCookie};
use http::{header, HeaderMap, HeaderValue};
use secrecy::SecretString;
use thiserror::Error;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_SESSIONS: usize = 64;
const MAX_SESSION_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_COOKIES: usize = 128;
const MAX_COOKIE_HEADER_BYTES: usize = 16 * 1024;
const MAX_PLATFORM_ID_BYTES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CookieJarSessionId(Uuid);

impl CookieJarSessionId {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, CookieJarSessionIdParseError> {
        let value = value.as_ref();
        let parsed = Uuid::parse_str(value).map_err(|_| CookieJarSessionIdParseError::Invalid)?;
        if parsed.to_string() != value {
            return Err(CookieJarSessionIdParseError::Invalid);
        }
        Ok(Self(parsed))
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CookieJarSessionIdParseError {
    #[error("Cookie Jar session ID must be a canonical UUID")]
    Invalid,
}

impl fmt::Debug for CookieJarSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CookieJarSessionId")
            .field(&"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for CookieJarSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieJarPurposeKind {
    Login,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CookieJarPurpose {
    Login,
    Refresh {
        credential_id: CredentialId,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieJarBinding {
    plugin_id: PluginId,
    manifest_hash: Option<String>,
    platform_id: String,
    scope: CredentialScope,
    target_origins: Vec<CredentialTargetOrigin>,
    purpose: CookieJarPurpose,
}

impl CookieJarBinding {
    pub fn login(
        plugin_id: PluginId,
        platform_id: impl Into<String>,
        scope: CredentialScope,
        target_origins: Vec<CredentialTargetOrigin>,
    ) -> Result<Self, CookieJarError> {
        Self::new(
            plugin_id,
            platform_id.into(),
            scope,
            target_origins,
            CookieJarPurpose::Login,
        )
    }

    pub fn refresh(
        plugin_id: PluginId,
        platform_id: impl Into<String>,
        scope: CredentialScope,
        target_origins: Vec<CredentialTargetOrigin>,
        credential_id: CredentialId,
        expected_revision: u64,
    ) -> Result<Self, CookieJarError> {
        if expected_revision == 0 {
            return Err(CookieJarError::InvalidBinding);
        }
        Self::new(
            plugin_id,
            platform_id.into(),
            scope,
            target_origins,
            CookieJarPurpose::Refresh {
                credential_id,
                expected_revision,
            },
        )
    }

    fn new(
        plugin_id: PluginId,
        platform_id: String,
        scope: CredentialScope,
        target_origins: Vec<CredentialTargetOrigin>,
        purpose: CookieJarPurpose,
    ) -> Result<Self, CookieJarError> {
        if platform_id.is_empty()
            || platform_id.len() > MAX_PLATFORM_ID_BYTES
            || !platform_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(CookieJarError::InvalidBinding);
        }
        let target_origins = normalize_origins(&target_origins)?;
        Ok(Self {
            plugin_id,
            manifest_hash: None,
            platform_id,
            scope,
            target_origins,
            purpose,
        })
    }

    pub fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    pub(crate) fn with_manifest_hash(mut self, manifest_hash: String) -> Self {
        self.manifest_hash = Some(manifest_hash);
        self
    }

    pub(crate) fn manifest_hash(&self) -> Option<&str> {
        self.manifest_hash.as_deref()
    }

    pub fn platform_id(&self) -> &str {
        &self.platform_id
    }

    pub fn scope(&self) -> &CredentialScope {
        &self.scope
    }

    pub fn target_origins(&self) -> &[CredentialTargetOrigin] {
        &self.target_origins
    }

    pub fn purpose(&self) -> CookieJarPurposeKind {
        match self.purpose {
            CookieJarPurpose::Login => CookieJarPurposeKind::Login,
            CookieJarPurpose::Refresh { .. } => CookieJarPurposeKind::Refresh,
        }
    }

    pub(crate) fn refresh_identity(&self) -> Option<(CredentialId, u64)> {
        match self.purpose {
            CookieJarPurpose::Login => None,
            CookieJarPurpose::Refresh {
                credential_id,
                expected_revision,
            } => Some((credential_id, expected_revision)),
        }
    }
}

pub struct PromotionSnapshot {
    target_origins: Vec<CredentialTargetOrigin>,
    secret: CookieCredentialSecret,
}

impl PromotionSnapshot {
    pub fn target_origins(&self) -> &[CredentialTargetOrigin] {
        &self.target_origins
    }

    pub fn secret(&self) -> &CookieCredentialSecret {
        &self.secret
    }

    pub fn into_secret(self) -> CookieCredentialSecret {
        self.secret
    }
}

impl fmt::Debug for PromotionSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PromotionSnapshot")
            .field("target_origin_count", &self.target_origins.len())
            .field("cookie_count", &self.secret.cookies().len())
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Default)]
pub struct TemporaryCookieJars {
    sessions: Arc<Mutex<HashMap<CookieJarSessionId, CookieJarSession>>>,
}

impl TemporaryCookieJars {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(
        &self,
        binding: CookieJarBinding,
        ttl: Duration,
    ) -> Result<CookieJarSessionId, CookieJarError> {
        self.create_with_store(binding, ttl, CookieStore::default())
    }

    pub(crate) fn create_seeded(
        &self,
        binding: CookieJarBinding,
        ttl: Duration,
        secret: &CookieCredentialSecret,
    ) -> Result<CookieJarSessionId, CookieJarError> {
        let mut store = CookieStore::default();
        for cookie in secret.cookies() {
            if cookie
                .expires_at()
                .is_some_and(|expires_at| expires_at <= Utc::now())
            {
                continue;
            }
            let origin = binding
                .target_origins
                .iter()
                .filter_map(|origin| Url::parse(origin.as_str()).ok())
                .find(|origin| {
                    origin
                        .host_str()
                        .is_some_and(|host| host.eq_ignore_ascii_case(cookie.host()))
                        && (!cookie.secure() || origin.scheme() == "https")
                })
                .ok_or(CookieJarError::OriginDenied)?;
            let raw = cookie.with_value(|value| seeded_raw_cookie(cookie, value))?;
            store
                .insert_raw(&raw, &origin)
                .map_err(|_| CookieJarError::InvalidCookie)?;
        }
        enforce_store_limits(&store)?;
        self.create_with_store(binding, ttl, store)
    }

    fn create_with_store(
        &self,
        binding: CookieJarBinding,
        ttl: Duration,
        store: CookieStore,
    ) -> Result<CookieJarSessionId, CookieJarError> {
        if ttl > MAX_SESSION_TTL {
            return Err(CookieJarError::InvalidBinding);
        }
        let now = Instant::now();
        let mut sessions = self.lock_sessions()?;
        sessions.retain(|_, session| session.expires_at > now);
        if sessions.len() >= MAX_SESSIONS {
            return Err(CookieJarError::CapacityReached);
        }
        let id = loop {
            let id = CookieJarSessionId(Uuid::new_v4());
            if !sessions.contains_key(&id) {
                break id;
            }
        };
        sessions.insert(
            id,
            CookieJarSession {
                binding,
                expires_at: now + ttl,
                store,
                state: SessionState::Active,
            },
        );
        Ok(id)
    }

    pub fn cancel(
        &self,
        id: &CookieJarSessionId,
        binding: &CookieJarBinding,
    ) -> Result<(), CookieJarError> {
        let mut sessions = self.lock_sessions()?;
        let session = live_session(&mut sessions, id)?;
        ensure_binding(session, binding)?;
        sessions.remove(id);
        Ok(())
    }

    pub fn capture_response(
        &self,
        id: &CookieJarSessionId,
        binding: &CookieJarBinding,
        response_url: &Url,
        headers: &HeaderMap,
    ) -> Result<(), CookieJarError> {
        self.capture_response_with_markers(id, binding, response_url, headers)
            .map(drop)
    }

    pub(crate) fn capture_response_with_markers(
        &self,
        id: &CookieJarSessionId,
        binding: &CookieJarBinding,
        response_url: &Url,
        headers: &HeaderMap,
    ) -> Result<Vec<Zeroizing<Vec<u8>>>, CookieJarError> {
        let mut sessions = self.lock_sessions()?;
        let session = live_session(&mut sessions, id)?;
        ensure_binding(session, binding)?;
        ensure_active(session)?;
        ensure_origin(binding, response_url)?;

        let mut updated = session.store.clone();
        let mut markers = Vec::new();
        for value in headers.get_all(header::SET_COOKIE) {
            let value = value.to_str().map_err(|_| CookieJarError::InvalidCookie)?;
            let raw =
                RawCookie::parse(value.to_owned()).map_err(|_| CookieJarError::InvalidCookie)?;
            if raw.name().is_empty() {
                return Err(CookieJarError::InvalidCookie);
            }
            reject_public_suffix(&raw)?;
            match updated.insert_raw(&raw, response_url) {
                Ok(_) | Err(CookieError::Expired) => {}
                Err(_) => return Err(CookieJarError::InvalidCookie),
            }
            markers.extend(cookie_secret_markers(&raw));
        }
        enforce_store_limits(&updated)?;
        session.store = updated;
        Ok(markers)
    }

    pub fn cookie_header(
        &self,
        id: &CookieJarSessionId,
        binding: &CookieJarBinding,
        request_url: &Url,
    ) -> Result<Option<HeaderValue>, CookieJarError> {
        let mut sessions = self.lock_sessions()?;
        let session = live_session(&mut sessions, id)?;
        ensure_binding(session, binding)?;
        ensure_active(session)?;
        ensure_origin(binding, request_url)?;
        cookie_header(&session.store, request_url)
    }

    pub fn promotion_snapshot(
        &self,
        id: &CookieJarSessionId,
        binding: &CookieJarBinding,
        selected_origins: &[CredentialTargetOrigin],
    ) -> Result<PromotionSnapshot, CookieJarError> {
        if binding.purpose() != CookieJarPurposeKind::Login {
            return Err(CookieJarError::PurposeMismatch);
        }
        let selected_origins = normalize_origins(selected_origins)?;
        if selected_origins.iter().any(|origin| {
            !binding
                .target_origins
                .iter()
                .any(|allowed| allowed == origin)
        }) {
            return Err(CookieJarError::OriginDenied);
        }
        let mut sessions = self.lock_sessions()?;
        let session = live_session(&mut sessions, id)?;
        ensure_binding(session, binding)?;
        ensure_active(session)?;
        let secret = snapshot_secret(&session.store, &selected_origins)?;
        Ok(PromotionSnapshot {
            target_origins: selected_origins,
            secret,
        })
    }

    pub(crate) fn binding(
        &self,
        id: &CookieJarSessionId,
    ) -> Result<CookieJarBinding, CookieJarError> {
        let mut sessions = self.lock_sessions()?;
        let session = live_session(&mut sessions, id)?;
        ensure_active(session)?;
        Ok(session.binding.clone())
    }

    pub(crate) fn begin_refresh_commit(
        &self,
        id: &CookieJarSessionId,
        binding: &CookieJarBinding,
        selected_origins: &[CredentialTargetOrigin],
    ) -> Result<RefreshCommitSnapshot, CookieJarError> {
        let selected_origins = normalize_origins(selected_origins)?;
        if selected_origins
            .iter()
            .any(|origin| !binding.target_origins.contains(origin))
        {
            return Err(CookieJarError::OriginDenied);
        }
        let mut sessions = self.lock_sessions()?;
        let session = live_session(&mut sessions, id)?;
        ensure_binding(session, binding)?;
        ensure_active(session)?;
        let (credential_id, expected_revision) = binding
            .refresh_identity()
            .ok_or(CookieJarError::PurposeMismatch)?;
        let secret = snapshot_secret(&session.store, &selected_origins)?;
        session.state = SessionState::Committing;
        Ok(RefreshCommitSnapshot {
            credential_id,
            expected_revision,
            target_origins: selected_origins,
            secret,
        })
    }

    pub(crate) fn finish_refresh(
        &self,
        id: &CookieJarSessionId,
        retryable: bool,
    ) -> Result<(), CookieJarError> {
        let mut sessions = self.lock_sessions()?;
        let Some(session) = sessions.get_mut(id) else {
            return Err(CookieJarError::NotFound);
        };
        if retryable {
            session.state = SessionState::Active;
        } else {
            sessions.remove(id);
        }
        Ok(())
    }

    fn lock_sessions(
        &self,
    ) -> Result<MutexGuard<'_, HashMap<CookieJarSessionId, CookieJarSession>>, CookieJarError> {
        self.sessions
            .lock()
            .map_err(|_| CookieJarError::Unavailable)
    }
}

pub(crate) struct RefreshCommitSnapshot {
    pub credential_id: CredentialId,
    pub expected_revision: u64,
    pub target_origins: Vec<CredentialTargetOrigin>,
    pub secret: CookieCredentialSecret,
}

struct CookieJarSession {
    binding: CookieJarBinding,
    expires_at: Instant,
    store: CookieStore,
    state: SessionState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Active,
    Committing,
}

fn live_session<'a>(
    sessions: &'a mut HashMap<CookieJarSessionId, CookieJarSession>,
    id: &CookieJarSessionId,
) -> Result<&'a mut CookieJarSession, CookieJarError> {
    let Some(session) = sessions.get(id) else {
        return Err(CookieJarError::NotFound);
    };
    if session.expires_at <= Instant::now() {
        sessions.remove(id);
        return Err(CookieJarError::Expired);
    }
    sessions.get_mut(id).ok_or(CookieJarError::NotFound)
}

fn ensure_binding(
    session: &CookieJarSession,
    binding: &CookieJarBinding,
) -> Result<(), CookieJarError> {
    if &session.binding == binding {
        Ok(())
    } else {
        Err(CookieJarError::BindingMismatch)
    }
}

fn ensure_active(session: &CookieJarSession) -> Result<(), CookieJarError> {
    if session.state == SessionState::Active {
        Ok(())
    } else {
        Err(CookieJarError::Busy)
    }
}

fn ensure_origin(binding: &CookieJarBinding, url: &Url) -> Result<(), CookieJarError> {
    let origin = CredentialTargetOrigin::parse(url.origin().ascii_serialization())
        .map_err(|_| CookieJarError::OriginDenied)?;
    if binding
        .target_origins
        .iter()
        .any(|allowed| allowed == &origin)
    {
        Ok(())
    } else {
        Err(CookieJarError::OriginDenied)
    }
}

fn cookie_header(
    store: &CookieStore,
    request_url: &Url,
) -> Result<Option<HeaderValue>, CookieJarError> {
    let mut matches = store.matches(request_url);
    matches.sort_by(|left, right| {
        right
            .path
            .len()
            .cmp(&left.path.len())
            .then_with(|| left.domain.cmp(&right.domain))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.name().cmp(right.name()))
    });
    if matches.len() > MAX_COOKIES {
        return Err(CookieJarError::HeaderTooLarge);
    }
    let mut value = Zeroizing::new(String::new());
    for cookie in matches {
        if !value.is_empty() {
            value.push_str("; ");
        }
        value.push_str(cookie.name());
        value.push('=');
        value.push_str(cookie.value());
        if value.len() > MAX_COOKIE_HEADER_BYTES {
            return Err(CookieJarError::HeaderTooLarge);
        }
    }
    if value.is_empty() {
        Ok(None)
    } else {
        HeaderValue::from_str(&value)
            .map(Some)
            .map_err(|_| CookieJarError::InvalidCookie)
    }
}

fn reject_public_suffix(raw: &RawCookie<'_>) -> Result<(), CookieJarError> {
    let Some(domain) = raw.domain() else {
        return Ok(());
    };
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    let canonical = Url::parse(&format!("https://{domain}/"))
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or(CookieJarError::InvalidCookie)?;
    if psl::suffix_str(&canonical).is_some_and(|suffix| suffix == canonical) {
        Err(CookieJarError::InvalidCookie)
    } else {
        Ok(())
    }
}

fn cookie_secret_markers(raw: &RawCookie<'_>) -> Vec<Zeroizing<Vec<u8>>> {
    let mut markers = Vec::new();
    let value = raw.value();
    if !value.is_empty() {
        markers.push(Zeroizing::new(value.as_bytes().to_vec()));

        let mut pair = Zeroizing::new(Vec::with_capacity(raw.name().len() + value.len() + 1));
        pair.extend_from_slice(raw.name().as_bytes());
        pair.push(b'=');
        pair.extend_from_slice(value.as_bytes());
        markers.push(pair);
    }
    let trimmed = raw.value_trimmed();
    if !trimmed.is_empty() && trimmed != value {
        markers.push(Zeroizing::new(trimmed.as_bytes().to_vec()));
    }
    markers
}

fn enforce_store_limits(store: &CookieStore) -> Result<(), CookieJarError> {
    let mut count = 0_usize;
    let mut bytes = 0_usize;
    for cookie in store.iter_unexpired() {
        count = count.checked_add(1).ok_or(CookieJarError::HeaderTooLarge)?;
        bytes = bytes
            .checked_add(cookie.name().len())
            .and_then(|value| value.checked_add(cookie.value().len()))
            .and_then(|value| value.checked_add(cookie.path.len()))
            .ok_or(CookieJarError::HeaderTooLarge)?;
    }
    if count > MAX_COOKIES || bytes > MAX_COOKIE_HEADER_BYTES * 2 {
        Err(CookieJarError::HeaderTooLarge)
    } else {
        Ok(())
    }
}

fn snapshot_secret(
    store: &CookieStore,
    selected_origins: &[CredentialTargetOrigin],
) -> Result<CookieCredentialSecret, CookieJarError> {
    let selected_urls = selected_origins
        .iter()
        .filter_map(|origin| Url::parse(origin.as_str()).ok())
        .collect::<Vec<_>>();
    let mut cookies = Vec::new();
    for cookie in store.iter_unexpired() {
        let mut exact_hosts = selected_urls
            .iter()
            .filter(|origin| {
                cookie.domain.matches(origin)
                    && (!cookie.secure().unwrap_or(false) || origin.scheme() == "https")
            })
            .filter_map(|origin| origin.host_str().map(str::to_ascii_lowercase))
            .collect::<Vec<_>>();
        exact_hosts.sort();
        exact_hosts.dedup();
        if exact_hosts.is_empty() {
            return Err(CookieJarError::OriginDenied);
        }
        let expires_at = cookie
            .expires_datetime()
            .and_then(|expires_at| DateTime::<Utc>::from_timestamp(expires_at.unix_timestamp(), 0));
        for host in exact_hosts {
            cookies.push(
                CookieSecretRecord::new(
                    cookie.name(),
                    SecretString::new(cookie.value().to_owned()),
                    host,
                    cookie.path.as_ref(),
                    cookie.secure().unwrap_or(false),
                    cookie.http_only().unwrap_or(false),
                    expires_at,
                )
                .map_err(map_secret_error)?,
            );
        }
    }
    CookieCredentialSecret::new(cookies).map_err(map_secret_error)
}

fn seeded_raw_cookie(
    cookie: &CookieSecretRecord,
    value: &str,
) -> Result<RawCookie<'static>, CookieJarError> {
    let mut encoded = Zeroizing::new(format!("{}={value}; Path={}", cookie.name(), cookie.path()));
    if cookie.secure() {
        encoded.push_str("; Secure");
    }
    if cookie.http_only() {
        encoded.push_str("; HttpOnly");
    }
    if let Some(expires_at) = cookie.expires_at() {
        let seconds = (expires_at - Utc::now()).num_seconds();
        if seconds <= 0 {
            return Err(CookieJarError::Expired);
        }
        encoded.push_str(&format!("; Max-Age={seconds}"));
    }
    let raw = RawCookie::parse(encoded.as_str())
        .map(RawCookie::into_owned)
        .map_err(|_| CookieJarError::InvalidCookie)?;
    if raw.name() != cookie.name() || raw.value() != value {
        return Err(CookieJarError::InvalidCookie);
    }
    Ok(raw)
}

fn normalize_origins(
    origins: &[CredentialTargetOrigin],
) -> Result<Vec<CredentialTargetOrigin>, CookieJarError> {
    if origins.is_empty() || origins.len() > 16 {
        return Err(CookieJarError::InvalidBinding);
    }
    let mut normalized = origins
        .iter()
        .map(|origin| {
            CredentialTargetOrigin::parse(origin.as_str())
                .map_err(|_| CookieJarError::InvalidBinding)
        })
        .collect::<Result<Vec<_>, _>>()?;
    normalized.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    normalized.dedup();
    if normalized.is_empty() {
        Err(CookieJarError::InvalidBinding)
    } else {
        Ok(normalized)
    }
}

fn map_secret_error(_error: SecretPayloadError) -> CookieJarError {
    CookieJarError::InvalidCookie
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CookieJarError {
    #[error("Cookie Jar binding is invalid")]
    InvalidBinding,
    #[error("Cookie Jar capacity is exhausted")]
    CapacityReached,
    #[error("Cookie Jar was not found")]
    NotFound,
    #[error("Cookie Jar has expired")]
    Expired,
    #[error("Cookie Jar binding does not match")]
    BindingMismatch,
    #[error("Cookie Jar purpose does not match")]
    PurposeMismatch,
    #[error("Cookie Jar origin is denied")]
    OriginDenied,
    #[error("Cookie Jar rejected a Cookie")]
    InvalidCookie,
    #[error("Cookie header is too large")]
    HeaderTooLarge,
    #[error("Cookie Jar is busy")]
    Busy,
    #[error("Cookie Jar is unavailable")]
    Unavailable,
}
