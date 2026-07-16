#![forbid(unsafe_code)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, HeaderMap, StatusCode, Uri},
    response::Response,
    routing::post,
    Router,
};
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UnixStream},
    sync::Semaphore,
    time::timeout,
};
use tower::ServiceExt;

pub const GATEWAY_LISTEN_ADDRESS: &str = "0.0.0.0:18081";
pub const CORE_BACKEND_SOCKET: &str = "/run/audiodown-proxy/core.sock";
pub const MAX_PROXY_FRAME_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const READ_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct GatewayLimits {
    pub body_timeout: Duration,
    pub server_timeout: Duration,
    pub max_concurrency: usize,
}

impl Default for GatewayLimits {
    fn default() -> Self {
        Self {
            body_timeout: Duration::from_secs(5),
            server_timeout: Duration::from_secs(12),
            max_concurrency: 64,
        }
    }
}

#[derive(Clone)]
struct GatewayState {
    backend_socket: Arc<PathBuf>,
    limits: GatewayLimits,
}

pub async fn run() -> Result<(), GatewayError> {
    let listener = TcpListener::bind(GATEWAY_LISTEN_ADDRESS)
        .await
        .map_err(|_| GatewayError::Bind)?;
    serve(listener, PathBuf::from(CORE_BACKEND_SOCKET)).await
}

pub async fn serve(listener: TcpListener, backend_socket: PathBuf) -> Result<(), GatewayError> {
    serve_with_limits(listener, backend_socket, GatewayLimits::default()).await
}

pub async fn serve_with_limits(
    listener: TcpListener,
    backend_socket: PathBuf,
    limits: GatewayLimits,
) -> Result<(), GatewayError> {
    if limits.body_timeout.is_zero()
        || limits.server_timeout.is_zero()
        || limits.max_concurrency == 0
    {
        return Err(GatewayError::InvalidLimits);
    }
    let state = GatewayState {
        backend_socket: Arc::new(backend_socket),
        limits,
    };
    let router = Router::new().route("/", post(relay)).with_state(state);
    let concurrency = Arc::new(Semaphore::new(limits.max_concurrency));
    loop {
        let (stream, _) = listener.accept().await.map_err(|_| GatewayError::Serve)?;
        let permit = match concurrency.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                reject_busy_connection(stream).await;
                continue;
            }
        };
        let router = router.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let router = router.clone();
                async move { router.oneshot(request.map(Body::new)).await }
            });
            let mut builder = http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .keep_alive(false)
                .header_read_timeout(limits.server_timeout);
            let _permit = permit;
            let _ = builder
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn reject_busy_connection(mut stream: TcpStream) {
    let mut request = [0_u8; 4096];
    let _ = timeout(Duration::from_millis(10), stream.read(&mut request)).await;
    while stream.try_read(&mut request).is_ok_and(|read| read > 0) {}
    const BODY: &[u8] = br#"{"status":503,"headers":{},"bodyBase64":null,"error":{"code":"GATEWAY_BUSY","summary":"Gateway concurrency limit was reached"}}"#;
    let head = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        BODY.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(BODY).await;
    let _ = stream.shutdown().await;
}

async fn relay(
    State(state): State<GatewayState>,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    match timeout(
        state.limits.server_timeout,
        relay_request(state.clone(), uri, headers, body),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => safe_error(
            StatusCode::GATEWAY_TIMEOUT,
            "GATEWAY_TIMEOUT",
            "Gateway request timed out",
        ),
    }
}

async fn relay_request(
    state: GatewayState,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    if uri.path() != "/" || uri.query().is_some() || !is_json_content_type(&headers) {
        return safe_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Gateway request was invalid",
        );
    }
    let frame = match timeout(
        state.limits.body_timeout,
        to_bytes(body, MAX_PROXY_FRAME_BYTES + 1),
    )
    .await
    {
        Ok(Ok(frame)) if frame.len() <= MAX_PROXY_FRAME_BYTES => frame,
        Err(_) => {
            return safe_error(
                StatusCode::REQUEST_TIMEOUT,
                "REQUEST_TIMEOUT",
                "Gateway request body timed out",
            )
        }
        _ => {
            return safe_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "MESSAGE_TOO_LARGE",
                "Gateway request exceeded the message limit",
            )
        }
    };
    if frame.is_empty()
        || frame.iter().any(|byte| matches!(byte, b'\n' | b'\r'))
        || serde_json::from_slice::<serde_json::Value>(&frame).is_err()
    {
        return safe_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Gateway request was invalid",
        );
    }

    match timeout(IO_TIMEOUT, relay_frame(&state.backend_socket, &frame)).await {
        Ok(Ok(response)) => json_response(StatusCode::OK, response),
        Ok(Err(RelayError::ResponseTooLarge)) => safe_error(
            StatusCode::BAD_GATEWAY,
            "MESSAGE_TOO_LARGE",
            "Gateway response exceeded the message limit",
        ),
        Ok(Err(RelayError::InvalidResponse)) => safe_error(
            StatusCode::BAD_GATEWAY,
            "PROXY_RESPONSE_INVALID",
            "Gateway response was invalid",
        ),
        Ok(Err(RelayError::Unavailable)) | Err(_) => safe_error(
            StatusCode::BAD_GATEWAY,
            "PROXY_UNAVAILABLE",
            "Core proxy is unavailable",
        ),
    }
}

async fn relay_frame(socket_path: &PathBuf, frame: &[u8]) -> Result<Vec<u8>, RelayError> {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .map_err(|_| RelayError::Unavailable)?;
    stream
        .write_all(frame)
        .await
        .map_err(|_| RelayError::Unavailable)?;
    stream
        .write_all(b"\n")
        .await
        .map_err(|_| RelayError::Unavailable)?;
    stream
        .shutdown()
        .await
        .map_err(|_| RelayError::Unavailable)?;
    read_frame(&mut stream).await
}

async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, RelayError> {
    let mut frame = Vec::with_capacity(READ_BUFFER_BYTES);
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|_| RelayError::InvalidResponse)?;
        if read == 0 {
            return Err(RelayError::InvalidResponse);
        }
        if let Some(newline) = buffer[..read].iter().position(|byte| *byte == b'\n') {
            if frame.len() + newline > MAX_PROXY_FRAME_BYTES {
                return Err(RelayError::ResponseTooLarge);
            }
            if newline + 1 != read {
                return Err(RelayError::InvalidResponse);
            }
            frame.extend_from_slice(&buffer[..newline]);
            if frame.is_empty()
                || frame.contains(&b'\r')
                || serde_json::from_slice::<serde_json::Value>(&frame).is_err()
            {
                return Err(RelayError::InvalidResponse);
            }
            let mut trailing = [0_u8; 1];
            return match stream.read(&mut trailing).await {
                Ok(0) => Ok(frame),
                Ok(_) | Err(_) => Err(RelayError::InvalidResponse),
            };
        }
        let remaining = MAX_PROXY_FRAME_BYTES
            .saturating_add(1)
            .saturating_sub(frame.len());
        frame.extend_from_slice(&buffer[..read.min(remaining)]);
        if frame.len() > MAX_PROXY_FRAME_BYTES || read > remaining {
            return Err(RelayError::ResponseTooLarge);
        }
    }
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn safe_error(status: StatusCode, code: &'static str, summary: &'static str) -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({
        "status": status.as_u16(),
        "headers": {},
        "bodyBase64": null,
        "error": {"code": code, "summary": summary}
    }))
    .unwrap_or_else(|_| {
        br#"{"status":502,"headers":{},"bodyBase64":null,"error":{"code":"PROXY_UNAVAILABLE","summary":"Core proxy is unavailable"}}"#.to_vec()
    });
    json_response(status, body)
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONNECTION, "close")
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayError {
    Unavailable,
    InvalidResponse,
    ResponseTooLarge,
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("fixed Gateway limits were invalid")]
    InvalidLimits,
    #[error("fixed Gateway listener could not bind")]
    Bind,
    #[error("fixed Gateway server failed")]
    Serve,
}
