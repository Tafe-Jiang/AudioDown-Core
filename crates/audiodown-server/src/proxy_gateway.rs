use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use async_trait::async_trait;
use audiodown_domain::{credential::CredentialScope, plugin::PluginId};
use audiodown_network_proxy::cookie_jar::CookieJarSessionId;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use base64::{engine::general_purpose, Engine as _};
use rand_core::{OsRng, RngCore};
use secrecy::{ExposeSecret, SecretString};
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{watch, Semaphore},
    task::JoinSet,
    time::timeout,
};
use uuid::Uuid;

pub const MAX_PROXY_FRAME_BYTES: usize = 1024 * 1024;
const MAX_TOKEN_BYTES: usize = 4 * 1024;
const MAX_REQUEST_ID_BYTES: usize = 256;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_REQUEST_HEADERS: usize = 32;
const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_NAME_BYTES: usize = 128;
const MAX_HEADER_VALUE_BYTES: usize = 8 * 1024;
const DEFAULT_REGISTRY_CAPACITY: usize = 256;
const READ_BUFFER_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct ProxyGatewayLimits {
    pub framing_timeout: Duration,
    pub write_timeout: Duration,
    pub max_connections: usize,
}

impl Default for ProxyGatewayLimits {
    fn default() -> Self {
        Self {
            framing_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            max_connections: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeGeneration(Uuid);

#[derive(Clone)]
pub struct ProxyToken(SecretString);

impl ProxyToken {
    pub fn with_value<T>(&self, operation: impl FnOnce(&str) -> T) -> T {
        operation(self.0.expose_secret())
    }
}

impl fmt::Debug for ProxyToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ProxyToken")
            .field(&"[REDACTED]")
            .finish()
    }
}

#[derive(Clone)]
pub struct RegisteredProxyToken {
    generation: RuntimeGeneration,
    token: ProxyToken,
}

impl RegisteredProxyToken {
    pub fn generation(&self) -> RuntimeGeneration {
        self.generation
    }

    pub fn token(&self) -> &ProxyToken {
        &self.token
    }
}

impl fmt::Debug for RegisteredProxyToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredProxyToken")
            .field("generation", &self.generation)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedRuntime {
    plugin_id: PluginId,
    generation: RuntimeGeneration,
}

impl AuthenticatedRuntime {
    pub fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    pub fn generation(&self) -> RuntimeGeneration {
        self.generation
    }
}

#[derive(Clone)]
pub struct ProxyTokenRegistry {
    capacity: usize,
    inner: Arc<RwLock<RegistryState>>,
}

#[derive(Default)]
struct RegistryState {
    by_digest: HashMap<[u8; 32], AuthenticatedRuntime>,
    by_plugin: HashMap<PluginId, (RuntimeGeneration, [u8; 32])>,
}

impl ProxyTokenRegistry {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_REGISTRY_CAPACITY).expect("default registry capacity is valid")
    }

    pub fn with_capacity(capacity: usize) -> Result<Self, ProxyRegistryError> {
        if capacity == 0 {
            return Err(ProxyRegistryError::InvalidCapacity);
        }
        Ok(Self {
            capacity,
            inner: Arc::new(RwLock::new(RegistryState::default())),
        })
    }

    pub fn register(
        &self,
        plugin_id: PluginId,
    ) -> Result<RegisteredProxyToken, ProxyRegistryError> {
        let mut bytes = [0_u8; 32];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ProxyRegistryError::RandomnessUnavailable)?;
        let token = ProxyToken(SecretString::new(
            general_purpose::URL_SAFE_NO_PAD.encode(bytes),
        ));
        let digest = token.with_value(token_digest);
        let generation = RuntimeGeneration(Uuid::new_v4());
        let runtime = AuthenticatedRuntime {
            plugin_id: plugin_id.clone(),
            generation,
        };

        let mut state = self
            .inner
            .write()
            .map_err(|_| ProxyRegistryError::Unavailable)?;
        if !state.by_plugin.contains_key(&plugin_id) && state.by_plugin.len() >= self.capacity {
            return Err(ProxyRegistryError::CapacityReached);
        }
        if let Some((_, previous_digest)) = state.by_plugin.remove(&plugin_id) {
            state.by_digest.remove(&previous_digest);
        }
        state.by_digest.insert(digest, runtime);
        state
            .by_plugin
            .insert(plugin_id.clone(), (generation, digest));
        drop(state);

        tracing::info!(plugin_id = %plugin_id, ?generation, "Proxy runtime token registered");
        Ok(RegisteredProxyToken { generation, token })
    }

    pub fn authenticate(&self, token: &str) -> Result<AuthenticatedRuntime, ProxyAuthError> {
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES || token.as_bytes().contains(&0) {
            return Err(ProxyAuthError::Unauthorized);
        }
        self.inner
            .read()
            .map_err(|_| ProxyAuthError::Unauthorized)?
            .by_digest
            .get(&token_digest(token))
            .cloned()
            .ok_or(ProxyAuthError::Unauthorized)
    }

    pub fn revoke(&self, plugin_id: &PluginId, generation: RuntimeGeneration) -> bool {
        let Ok(mut state) = self.inner.write() else {
            return false;
        };
        let Some((current_generation, digest)) = state.by_plugin.get(plugin_id).copied() else {
            return false;
        };
        if current_generation != generation {
            return false;
        }
        state.by_plugin.remove(plugin_id);
        state.by_digest.remove(&digest);
        true
    }

    pub fn revoke_plugin(&self, plugin_id: &PluginId) -> bool {
        let Ok(mut state) = self.inner.write() else {
            return false;
        };
        let Some((_, digest)) = state.by_plugin.remove(plugin_id) else {
            return false;
        };
        state.by_digest.remove(&digest);
        true
    }

    pub fn revoke_all(&self) {
        if let Ok(mut state) = self.inner.write() {
            state.by_digest.clear();
            state.by_plugin.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.inner.read().map_or(0, |state| state.by_plugin.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ProxyTokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProxyRegistryError {
    #[error("proxy token registry capacity is invalid")]
    InvalidCapacity,
    #[error("proxy token registry capacity was reached")]
    CapacityReached,
    #[error("operating system randomness is unavailable")]
    RandomnessUnavailable,
    #[error("proxy token registry is unavailable")]
    Unavailable,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProxyAuthError {
    #[error("proxy request is unauthorized")]
    Unauthorized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreProxyRequest {
    pub request_id: String,
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub cookie_jar_session_id: Option<CookieJarSessionId>,
    pub credential_scope: Option<CredentialScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreProxyResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl CoreProxyResponse {
    pub fn new(status: StatusCode, headers: HeaderMap, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CoreProxyBackendError {
    #[error("proxy request is invalid")]
    InvalidRequest,
    #[error("proxy policy denied the request")]
    PolicyDenied,
    #[error("credential scope is not allowed")]
    CredentialScopeNotAllowed,
    #[error("proxy backend is busy")]
    Busy,
    #[error("proxy backend timed out")]
    Timeout,
    #[error("proxy backend is unavailable")]
    Unavailable,
    #[error("proxy response was rejected")]
    ResponseRejected,
}

#[async_trait]
pub trait CoreProxyBackend: Send + Sync {
    async fn execute(
        &self,
        runtime: &AuthenticatedRuntime,
        request: CoreProxyRequest,
    ) -> Result<CoreProxyResponse, CoreProxyBackendError>;
}

pub struct ProxyGateway {
    listener: UnixListener,
    registry: Arc<ProxyTokenRegistry>,
    backend: Arc<dyn CoreProxyBackend>,
    limits: ProxyGatewayLimits,
    cleanup: SocketCleanup,
}

impl ProxyGateway {
    pub async fn bind(
        path: impl AsRef<Path>,
        registry: Arc<ProxyTokenRegistry>,
        backend: Arc<dyn CoreProxyBackend>,
    ) -> Result<Self, ProxyGatewayError> {
        Self::bind_with_limits(path, registry, backend, ProxyGatewayLimits::default()).await
    }

    #[doc(hidden)]
    pub async fn bind_with_limits(
        path: impl AsRef<Path>,
        registry: Arc<ProxyTokenRegistry>,
        backend: Arc<dyn CoreProxyBackend>,
        limits: ProxyGatewayLimits,
    ) -> Result<Self, ProxyGatewayError> {
        if limits.framing_timeout.is_zero()
            || limits.write_timeout.is_zero()
            || limits.max_connections == 0
        {
            return Err(ProxyGatewayError::InvalidLimits);
        }
        let path = path.as_ref().to_path_buf();
        prepare_socket_path(&path)?;
        let listener = UnixListener::bind(&path).map_err(|_| ProxyGatewayError::Bind)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                let _ = fs::remove_file(&path);
                return Err(ProxyGatewayError::Bind);
            }
        };
        let cleanup = SocketCleanup::new(path, &metadata);
        fs::set_permissions(cleanup.path(), fs::Permissions::from_mode(0o666))
            .map_err(|_| ProxyGatewayError::Permissions)?;
        Ok(Self {
            listener,
            registry,
            backend,
            limits,
            cleanup,
        })
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> Result<(), ProxyGatewayError> {
        let permits = Arc::new(Semaphore::new(self.limits.max_connections));
        let mut tasks = JoinSet::new();
        let result = loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break Ok(());
                    }
                }
                accepted = self.listener.accept() => {
                    let (stream, _) = match accepted {
                        Ok(value) => value,
                        Err(_) => break Err(ProxyGatewayError::Accept),
                    };
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                        drop(stream);
                        continue;
                    };
                    let registry = Arc::clone(&self.registry);
                    let backend = Arc::clone(&self.backend);
                    let limits = self.limits;
                    tasks.spawn(async move {
                        let _permit = permit;
                        handle_connection(stream, registry, backend, limits).await;
                    });
                }
                completed = tasks.join_next(), if !tasks.is_empty() => {
                    if completed.is_some_and(|result| result.is_err()) {
                        break Err(ProxyGatewayError::ConnectionTask);
                    }
                }
            }
        };

        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        self.registry.revoke_all();
        self.cleanup.remove_if_owned();
        result
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    registry: Arc<ProxyTokenRegistry>,
    backend: Arc<dyn CoreProxyBackend>,
    limits: ProxyGatewayLimits,
) {
    let response = match timeout(limits.framing_timeout, read_frame(&mut stream)).await {
        Ok(Ok(frame)) => dispatch_frame(&frame, &registry, backend.as_ref()).await,
        Ok(Err(FrameReadError::TooLarge)) => WireResponse::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "MESSAGE_TOO_LARGE",
            "Proxy request exceeded the message limit",
        ),
        Ok(Err(FrameReadError::Invalid)) | Err(_) => WireResponse::error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Proxy request was invalid",
        ),
    };
    let bytes = encode_response(response);
    let _ = timeout(limits.write_timeout, async {
        stream.write_all(&bytes).await?;
        stream.shutdown().await
    })
    .await;
}

async fn dispatch_frame(
    frame: &[u8],
    registry: &ProxyTokenRegistry,
    backend: &dyn CoreProxyBackend,
) -> WireResponse {
    let wire = match serde_json::from_slice::<WireRequest>(frame) {
        Ok(wire) => wire,
        Err(_) => {
            return WireResponse::error(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "Proxy request was invalid",
            )
        }
    };
    let runtime = match wire
        .token
        .as_deref()
        .ok_or(ProxyAuthError::Unauthorized)
        .and_then(|token| registry.authenticate(token))
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return WireResponse::error(
                StatusCode::UNAUTHORIZED,
                "PROXY_UNAUTHORIZED",
                "Proxy authentication failed",
            )
        }
    };
    let request = match wire.into_core_request() {
        Ok(request) => request,
        Err(()) => {
            return WireResponse::error(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "Proxy request was invalid",
            )
        }
    };
    match backend.execute(&runtime, request).await {
        Ok(response) => WireResponse::from_core(response).unwrap_or_else(|_| {
            WireResponse::error(
                StatusCode::BAD_GATEWAY,
                "PROXY_RESPONSE_REJECTED",
                "Proxy response was rejected",
            )
        }),
        Err(error) => map_backend_error(error),
    }
}

fn map_backend_error(error: CoreProxyBackendError) -> WireResponse {
    match error {
        CoreProxyBackendError::InvalidRequest => WireResponse::error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Proxy request was invalid",
        ),
        CoreProxyBackendError::PolicyDenied => WireResponse::error(
            StatusCode::FORBIDDEN,
            "PROXY_POLICY_DENIED",
            "Proxy policy denied the request",
        ),
        CoreProxyBackendError::CredentialScopeNotAllowed => WireResponse::error(
            StatusCode::FORBIDDEN,
            "CREDENTIAL_SCOPE_NOT_ALLOWED",
            "Credential scope is not allowed",
        ),
        CoreProxyBackendError::Busy => WireResponse::error(
            StatusCode::TOO_MANY_REQUESTS,
            "PROXY_BUSY",
            "Proxy capacity is currently unavailable",
        ),
        CoreProxyBackendError::Timeout => WireResponse::error(
            StatusCode::GATEWAY_TIMEOUT,
            "PROXY_TIMEOUT",
            "Proxy request timed out",
        ),
        CoreProxyBackendError::Unavailable => WireResponse::error(
            StatusCode::BAD_GATEWAY,
            "PROXY_UNAVAILABLE",
            "Proxy backend is unavailable",
        ),
        CoreProxyBackendError::ResponseRejected => WireResponse::error(
            StatusCode::BAD_GATEWAY,
            "PROXY_RESPONSE_REJECTED",
            "Proxy response was rejected",
        ),
    }
}

fn encode_response(response: WireResponse) -> Vec<u8> {
    let mut encoded = serde_json::to_vec(&response).unwrap_or_else(|_| fallback_response());
    if encoded.len() > MAX_PROXY_FRAME_BYTES {
        encoded = serde_json::to_vec(&WireResponse::error(
            StatusCode::BAD_GATEWAY,
            "MESSAGE_TOO_LARGE",
            "Proxy response exceeded the message limit",
        ))
        .unwrap_or_else(|_| fallback_response());
    }
    encoded.push(b'\n');
    encoded
}

fn fallback_response() -> Vec<u8> {
    br#"{"status":502,"headers":{},"bodyBase64":null,"error":{"code":"PROXY_UNAVAILABLE","summary":"Proxy backend is unavailable"}}"#.to_vec()
}

async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, FrameReadError> {
    let mut frame = Vec::with_capacity(READ_BUFFER_BYTES);
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|_| FrameReadError::Invalid)?;
        if read == 0 {
            return Err(FrameReadError::Invalid);
        }
        if let Some(newline) = buffer[..read].iter().position(|byte| *byte == b'\n') {
            if frame.len() + newline > MAX_PROXY_FRAME_BYTES || newline + 1 != read {
                return Err(if frame.len() + newline > MAX_PROXY_FRAME_BYTES {
                    FrameReadError::TooLarge
                } else {
                    FrameReadError::Invalid
                });
            }
            frame.extend_from_slice(&buffer[..newline]);
            return if frame.is_empty() {
                Err(FrameReadError::Invalid)
            } else {
                Ok(frame)
            };
        }
        let remaining = MAX_PROXY_FRAME_BYTES
            .saturating_add(1)
            .saturating_sub(frame.len());
        frame.extend_from_slice(&buffer[..read.min(remaining)]);
        if frame.len() > MAX_PROXY_FRAME_BYTES || read > remaining {
            return Err(FrameReadError::TooLarge);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FrameReadError {
    Invalid,
    TooLarge,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireRequest {
    token: Option<String>,
    request_id: String,
    method: String,
    url: String,
    #[serde(deserialize_with = "deserialize_headers")]
    headers: HashMap<String, String>,
    body_base64: Option<String>,
    cookie_jar_session_id: Option<String>,
    credential_scope: Option<String>,
}

fn deserialize_headers<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct HeaderVisitor;

    impl<'de> Visitor<'de> for HeaderVisitor {
        type Value = HashMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object with unique case-insensitive header names")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut headers = HashMap::new();
            let mut names = HashSet::new();
            while let Some((name, value)) = map.next_entry::<String, String>()? {
                if !names.insert(name.to_ascii_lowercase()) {
                    return Err(de::Error::custom("duplicate request header"));
                }
                headers.insert(name, value);
            }
            Ok(headers)
        }
    }

    deserializer.deserialize_map(HeaderVisitor)
}

impl WireRequest {
    fn into_core_request(self) -> Result<CoreProxyRequest, ()> {
        if self.request_id.is_empty()
            || self.request_id.len() > MAX_REQUEST_ID_BYTES
            || self.request_id.as_bytes().contains(&0)
            || self.url.is_empty()
            || self.url.len() > MAX_URL_BYTES
            || self.headers.len() > MAX_REQUEST_HEADERS
            || (self.cookie_jar_session_id.is_some() && self.credential_scope.is_some())
        {
            return Err(());
        }
        let method = Method::from_bytes(self.method.as_bytes()).map_err(|_| ())?;
        if !matches!(
            method,
            Method::GET
                | Method::HEAD
                | Method::POST
                | Method::PUT
                | Method::PATCH
                | Method::DELETE
        ) {
            return Err(());
        }
        let mut headers = HeaderMap::new();
        let mut normalized = HashSet::new();
        let mut header_bytes = 0_usize;
        for (raw_name, raw_value) in self.headers {
            if raw_name.is_empty()
                || raw_name.len() > MAX_HEADER_NAME_BYTES
                || raw_value.len() > MAX_HEADER_VALUE_BYTES
                || raw_name
                    .bytes()
                    .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
                || raw_value
                    .bytes()
                    .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
            {
                return Err(());
            }
            let lowercase = raw_name.to_ascii_lowercase();
            if !normalized.insert(lowercase.clone()) || forbidden_request_header(&lowercase) {
                return Err(());
            }
            header_bytes = header_bytes
                .checked_add(raw_name.len() + raw_value.len())
                .ok_or(())?;
            if header_bytes > MAX_REQUEST_HEADER_BYTES {
                return Err(());
            }
            let name = HeaderName::from_bytes(lowercase.as_bytes()).map_err(|_| ())?;
            let value = HeaderValue::from_str(&raw_value).map_err(|_| ())?;
            headers.insert(name, value);
        }
        let body = match self.body_base64 {
            Some(encoded) => {
                let decoded = general_purpose::STANDARD.decode(&encoded).map_err(|_| ())?;
                if general_purpose::STANDARD.encode(&decoded) != encoded {
                    return Err(());
                }
                decoded
            }
            None => Vec::new(),
        };
        let cookie_jar_session_id = self
            .cookie_jar_session_id
            .map(CookieJarSessionId::parse)
            .transpose()
            .map_err(|_| ())?;
        let credential_scope = self
            .credential_scope
            .map(CredentialScope::parse)
            .transpose()
            .map_err(|_| ())?;
        Ok(CoreProxyRequest {
            request_id: self.request_id,
            method,
            url: self.url,
            headers,
            body,
            cookie_jar_session_id,
            credential_scope,
        })
    }
}

fn forbidden_request_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "cookie"
            | "set-cookie"
            | "host"
            | "connection"
            | "content-length"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "forwarded"
            | "x-forwarded-for"
            | "x-forwarded-host"
            | "x-forwarded-proto"
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireResponse {
    status: u16,
    headers: HashMap<String, String>,
    body_base64: Option<String>,
    error: Option<WireError>,
}

impl WireResponse {
    fn from_core(response: CoreProxyResponse) -> Result<Self, ()> {
        let mut headers = HashMap::new();
        for (name, value) in &response.headers {
            let value = value.to_str().map_err(|_| ())?.to_string();
            if headers.insert(name.as_str().to_string(), value).is_some() {
                return Err(());
            }
        }
        Ok(Self {
            status: response.status.as_u16(),
            headers,
            body_base64: Some(general_purpose::STANDARD.encode(response.body)),
            error: None,
        })
    }

    fn error(status: StatusCode, code: &'static str, summary: &'static str) -> Self {
        Self {
            status: status.as_u16(),
            headers: HashMap::new(),
            body_base64: None,
            error: Some(WireError {
                code,
                summary,
                retry_after_seconds: None,
            }),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireError {
    code: &'static str,
    summary: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_seconds: Option<u64>,
}

fn prepare_socket_path(path: &Path) -> Result<(), ProxyGatewayError> {
    let parent = path.parent().ok_or(ProxyGatewayError::InvalidPath)?;
    fs::create_dir_all(parent).map_err(|_| ProxyGatewayError::Bind)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755))
        .map_err(|_| ProxyGatewayError::Permissions)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path).map_err(|_| ProxyGatewayError::UnsafeExistingPath)
        }
        Ok(_) => Err(ProxyGatewayError::UnsafeExistingPath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ProxyGatewayError::UnsafeExistingPath),
    }
}

struct SocketCleanup {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketCleanup {
    fn new(path: PathBuf, metadata: &fs::Metadata) -> Self {
        Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn remove_if_owned(&self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        self.remove_if_owned();
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProxyGatewayError {
    #[error("proxy gateway path is invalid")]
    InvalidPath,
    #[error("proxy gateway limits are invalid")]
    InvalidLimits,
    #[error("proxy gateway refused an existing path")]
    UnsafeExistingPath,
    #[error("proxy gateway socket could not be bound")]
    Bind,
    #[error("proxy gateway socket permissions could not be set")]
    Permissions,
    #[error("proxy gateway could not accept a connection")]
    Accept,
    #[error("proxy gateway connection task failed")]
    ConnectionTask,
}
