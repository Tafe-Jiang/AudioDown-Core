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
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixStream},
    time::timeout,
};

pub const GATEWAY_LISTEN_ADDRESS: &str = "0.0.0.0:18081";
pub const CORE_BACKEND_SOCKET: &str = "/run/audiodown-proxy/core.sock";
pub const MAX_PROXY_FRAME_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const READ_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Clone)]
struct GatewayState {
    backend_socket: Arc<PathBuf>,
}

pub async fn run() -> Result<(), GatewayError> {
    let listener = TcpListener::bind(GATEWAY_LISTEN_ADDRESS)
        .await
        .map_err(|_| GatewayError::Bind)?;
    serve(listener, PathBuf::from(CORE_BACKEND_SOCKET)).await
}

pub async fn serve(listener: TcpListener, backend_socket: PathBuf) -> Result<(), GatewayError> {
    let state = GatewayState {
        backend_socket: Arc::new(backend_socket),
    };
    let router = Router::new().route("/", post(relay)).with_state(state);
    axum::serve(listener, router)
        .await
        .map_err(|_| GatewayError::Serve)
}

async fn relay(
    State(state): State<GatewayState>,
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
    let frame = match to_bytes(body, MAX_PROXY_FRAME_BYTES + 1).await {
        Ok(frame) if frame.len() <= MAX_PROXY_FRAME_BYTES => frame,
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
            return Ok(frame);
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
    #[error("fixed Gateway listener could not bind")]
    Bind,
    #[error("fixed Gateway server failed")]
    Serve,
}
