use std::{
    io::Read,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use http::{header, HeaderMap, HeaderName, Method, StatusCode};
use tokio::{
    sync::Semaphore,
    task::spawn_blocking,
    time::{timeout_at, Instant},
};

use crate::{
    error::HttpProxyError,
    policy::{PinnedTarget, ProxyPolicy},
    resolver::DnsResolver,
};

const MAX_CONCURRENT_REQUESTS: usize = 8;
const MAX_CONCURRENT_DECODERS: usize = MAX_CONCURRENT_REQUESTS;
const MAX_REDIRECTS: usize = 5;
const MAX_REQUEST_HEADERS: usize = 32;
const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_HEADERS: usize = 64;
const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1024;
const MAX_RAW_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyRequest {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[doc(hidden)]
/// A complete normalized request for a trusted transport implementation.
///
/// This is an internal/test injection contract, not a security boundary.
/// Callers must enter through [`HttpProxy::execute`] to receive policy and
/// resource-limit enforcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportRequest {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[doc(hidden)]
/// A complete raw response returned by a trusted transport implementation.
///
/// `HttpProxy` validates and filters every field before exposing a
/// [`ProxyResponse`]. Direct construction does not grant that protection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[doc(hidden)]
/// A source-free failure from the trusted transport injection seam.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
#[error("HTTP transport failed")]
pub struct TransportError;

#[doc(hidden)]
/// Trusted internal/test injection seam beneath [`HttpProxy`].
///
/// Implementations receive an already-authorized [`PinnedTarget`], but this
/// trait does not independently enforce policy, pinning, deadlines, or size
/// limits. Security-sensitive callers must use [`HttpProxy::execute`].
#[async_trait]
pub trait HttpTransport: Send + Sync + 'static {
    async fn execute(
        &self,
        target: &PinnedTarget,
        request: TransportRequest,
    ) -> Result<TransportResponse, TransportError>;
}

pub struct HttpProxy<R, T> {
    policy: ProxyPolicy,
    resolver: Arc<Mutex<R>>,
    resolver_jobs: Arc<Semaphore>,
    decoder_jobs: Arc<Semaphore>,
    transport: T,
    requests: Arc<Semaphore>,
}

impl<R, T> HttpProxy<R, T>
where
    R: DnsResolver + Send + 'static,
    T: HttpTransport,
{
    pub fn new(policy: ProxyPolicy, resolver: R, transport: T) -> Self {
        Self {
            policy,
            resolver: Arc::new(Mutex::new(resolver)),
            resolver_jobs: Arc::new(Semaphore::new(1)),
            decoder_jobs: Arc::new(Semaphore::new(MAX_CONCURRENT_DECODERS)),
            transport,
            requests: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
        }
    }

    pub async fn execute(&self, request: ProxyRequest) -> Result<ProxyResponse, HttpProxyError> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let _permit = self
            .requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| HttpProxyError::ConcurrencyLimited)?;
        validate_method(&request.method)?;
        validate_request_headers(&request.headers)?;
        if request.body.len() > MAX_REQUEST_BODY_BYTES {
            return Err(HttpProxyError::RequestBodyTooLarge);
        }

        let mut target = self.authorize_url(request.url.clone(), deadline).await?;
        let initial_target = target.clone();
        let mut redirects = 0;

        loop {
            let transport_request = TransportRequest {
                method: request.method.clone(),
                url: target.url().to_string(),
                headers: request.headers.clone(),
                body: request.body.clone(),
            };
            let response = timeout_at(deadline, self.transport.execute(&target, transport_request))
                .await
                .map_err(|_| HttpProxyError::Timeout)?
                .map_err(|_| HttpProxyError::Transport)?;

            validate_response_headers(&response.headers)?;
            validate_response_framing(&response.headers)?;
            if response.body.len() > MAX_RAW_RESPONSE_BODY_BYTES {
                return Err(HttpProxyError::ResponseBodyTooLarge);
            }

            if is_redirect(response.status) {
                if redirects >= MAX_REDIRECTS {
                    return Err(HttpProxyError::TooManyRedirects);
                }
                if matches!(
                    response.status,
                    StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
                ) && !matches!(request.method, Method::GET | Method::HEAD)
                {
                    return Err(HttpProxyError::InvalidRedirect);
                }
                let location = one_location(&response.headers)?;
                let redirected = target
                    .url()
                    .join(location)
                    .map_err(|_| HttpProxyError::InvalidRedirect)?;
                if !matches!(redirected.scheme(), "http" | "https") {
                    return Err(HttpProxyError::InvalidRedirect);
                }
                target = self
                    .authorize_redirect(initial_target.clone(), redirected.to_string(), deadline)
                    .await?;
                redirects += 1;
                continue;
            }

            let filtered_headers = filter_response_headers(&response.headers);
            let body = self
                .decode_body(response.headers, response.body, deadline)
                .await?;
            return Ok(ProxyResponse {
                status: response.status,
                headers: filtered_headers,
                body,
            });
        }
    }

    async fn authorize_url(
        &self,
        raw_url: String,
        deadline: Instant,
    ) -> Result<PinnedTarget, HttpProxyError> {
        let permit = timeout_at(deadline, self.resolver_jobs.clone().acquire_owned())
            .await
            .map_err(|_| HttpProxyError::Timeout)?
            .map_err(|_| HttpProxyError::Transport)?;
        let policy = self.policy.clone();
        let resolver = self.resolver.clone();
        let task = spawn_blocking(move || {
            let _permit = permit;
            let mut resolver = resolver.lock().map_err(|_| HttpProxyError::Transport)?;
            policy
                .authorize_url(&raw_url, &mut *resolver)
                .map_err(HttpProxyError::from)
        });
        timeout_at(deadline, task)
            .await
            .map_err(|_| HttpProxyError::Timeout)?
            .map_err(|_| HttpProxyError::Transport)?
    }

    async fn authorize_redirect(
        &self,
        initial_target: PinnedTarget,
        raw_url: String,
        deadline: Instant,
    ) -> Result<PinnedTarget, HttpProxyError> {
        let permit = timeout_at(deadline, self.resolver_jobs.clone().acquire_owned())
            .await
            .map_err(|_| HttpProxyError::Timeout)?
            .map_err(|_| HttpProxyError::Transport)?;
        let policy = self.policy.clone();
        let resolver = self.resolver.clone();
        let task = spawn_blocking(move || {
            let _permit = permit;
            let mut resolver = resolver.lock().map_err(|_| HttpProxyError::Transport)?;
            policy
                .authorize_redirect(&initial_target, &raw_url, &mut *resolver)
                .map_err(HttpProxyError::from)
        });
        timeout_at(deadline, task)
            .await
            .map_err(|_| HttpProxyError::Timeout)?
            .map_err(|_| HttpProxyError::Transport)?
    }

    async fn decode_body(
        &self,
        headers: HeaderMap,
        body: Vec<u8>,
        deadline: Instant,
    ) -> Result<Vec<u8>, HttpProxyError> {
        let permit = timeout_at(deadline, self.decoder_jobs.clone().acquire_owned())
            .await
            .map_err(|_| HttpProxyError::Timeout)?
            .map_err(|_| HttpProxyError::Transport)?;
        let task = spawn_blocking(move || {
            let _permit = permit;
            decode_body(&headers, body)
        });
        timeout_at(deadline, task)
            .await
            .map_err(|_| HttpProxyError::Timeout)?
            .map_err(|_| HttpProxyError::Transport)?
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestTransport;

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn execute(
        &self,
        target: &PinnedTarget,
        request: TransportRequest,
    ) -> Result<TransportResponse, TransportError> {
        if request.url != target.url().as_str() {
            return Err(TransportError);
        }
        let addresses: Vec<_> = target
            .pinned_addresses()
            .iter()
            .map(|address| SocketAddr::new(*address, target.port()))
            .collect();
        let authority_host = target.url().host_str().ok_or(TransportError)?;
        // Reqwest 0.12 exposes no HTTP/1 parser-limit builder. Hyper's built-in
        // 100-header cap remains active; proxy byte/count checks run after parse.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .http2_max_header_list_size(MAX_RESPONSE_HEADER_BYTES as u32)
            .resolve_to_addrs(authority_host, &addresses)
            .build()
            .map_err(|_| TransportError)?;
        let response = client
            .request(request.method, request.url)
            .headers(request.headers)
            .body(request.body)
            .send()
            .await
            .map_err(|_| TransportError)?;
        let status = response.status();
        let headers = response.headers().clone();
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| TransportError)?;
            let remaining = MAX_RAW_RESPONSE_BODY_BYTES
                .saturating_add(1)
                .saturating_sub(body.len());
            body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            if body.len() > MAX_RAW_RESPONSE_BODY_BYTES {
                break;
            }
        }
        Ok(TransportResponse {
            status,
            headers,
            body,
        })
    }
}

fn validate_method(method: &Method) -> Result<(), HttpProxyError> {
    if matches!(
        *method,
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        Ok(())
    } else {
        Err(HttpProxyError::MethodNotAllowed)
    }
}

fn validate_request_headers(headers: &HeaderMap) -> Result<(), HttpProxyError> {
    if headers.len() > MAX_REQUEST_HEADERS
        || header_bytes(headers).is_none_or(|bytes| bytes > MAX_REQUEST_HEADER_BYTES)
        || headers.keys().any(forbidden_request_header)
    {
        Err(HttpProxyError::RequestHeadersTooLarge)
    } else {
        Ok(())
    }
}

fn forbidden_request_header(name: &HeaderName) -> bool {
    let name = name.as_str();
    matches!(
        name,
        "host"
            | "cookie"
            | "set-cookie"
            | "authorization"
            | "forwarded"
            | "x-real-ip"
            | "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "content-length"
            | "expect"
            | "te"
            | "trailer"
            | "upgrade"
            | "via"
    ) || name.starts_with("proxy-")
        || name.starts_with("x-forwarded-")
}

fn validate_response_headers(headers: &HeaderMap) -> Result<(), HttpProxyError> {
    if headers.len() > MAX_RESPONSE_HEADERS
        || header_bytes(headers).is_none_or(|bytes| bytes > MAX_RESPONSE_HEADER_BYTES)
    {
        Err(HttpProxyError::ResponseHeadersTooLarge)
    } else {
        Ok(())
    }
}

fn header_bytes(headers: &HeaderMap) -> Option<usize> {
    headers.iter().try_fold(0_usize, |total, (name, value)| {
        total
            .checked_add(name.as_str().len())?
            .checked_add(value.as_bytes().len())
    })
}

fn validate_response_framing(headers: &HeaderMap) -> Result<(), HttpProxyError> {
    let lengths: Vec<_> = headers.get_all(header::CONTENT_LENGTH).iter().collect();
    if lengths.len() > 1 || (!lengths.is_empty() && headers.contains_key(header::TRANSFER_ENCODING))
    {
        return Err(HttpProxyError::Transport);
    }
    if let Some(value) = lengths.first() {
        let value = value.to_str().map_err(|_| HttpProxyError::Transport)?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(HttpProxyError::Transport);
        }
        let length = value
            .parse::<u64>()
            .map_err(|_| HttpProxyError::Transport)?;
        if length > MAX_RAW_RESPONSE_BODY_BYTES as u64 {
            return Err(HttpProxyError::ResponseBodyTooLarge);
        }
    }
    Ok(())
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn one_location(headers: &HeaderMap) -> Result<&str, HttpProxyError> {
    let values: Vec<_> = headers.get_all(header::LOCATION).iter().collect();
    if values.len() != 1 {
        return Err(HttpProxyError::InvalidRedirect);
    }
    values[0]
        .to_str()
        .map_err(|_| HttpProxyError::InvalidRedirect)
}

fn decode_body(headers: &HeaderMap, body: Vec<u8>) -> Result<Vec<u8>, HttpProxyError> {
    let encodings: Vec<_> = headers.get_all(header::CONTENT_ENCODING).iter().collect();
    if encodings.len() > 1 {
        return Err(HttpProxyError::InvalidResponseEncoding);
    }
    let Some(encoding) = encodings.first() else {
        return Ok(body);
    };
    let encoding = encoding
        .to_str()
        .map_err(|_| HttpProxyError::InvalidResponseEncoding)?;
    if encoding.eq_ignore_ascii_case("identity") {
        return Ok(body);
    }
    if !encoding.eq_ignore_ascii_case("gzip") || encoding.contains(',') {
        return Err(HttpProxyError::InvalidResponseEncoding);
    }

    let mut decoded = Vec::new();
    GzDecoder::new(body.as_slice())
        .take((MAX_RESPONSE_BODY_BYTES + 1) as u64)
        .read_to_end(&mut decoded)
        .map_err(|_| HttpProxyError::InvalidResponseEncoding)?;
    if decoded.len() > MAX_RESPONSE_BODY_BYTES {
        return Err(HttpProxyError::ResponseBodyTooLarge);
    }
    Ok(decoded)
}

fn filter_response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut filtered = HeaderMap::new();
    for (name, value) in headers {
        if allowed_response_header(name) {
            filtered.append(name.clone(), value.clone());
        }
    }
    filtered
}

fn allowed_response_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "accept-ranges"
            | "cache-control"
            | "content-language"
            | "content-range"
            | "content-type"
            | "etag"
            | "expires"
            | "last-modified"
            | "retry-after"
            | "vary"
    )
}
