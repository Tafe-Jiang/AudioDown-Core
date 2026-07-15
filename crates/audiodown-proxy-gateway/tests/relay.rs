use std::{future::Future, net::SocketAddr, path::PathBuf};

use audiodown_proxy_gateway::{serve_with_limits, GatewayLimits, MAX_PROXY_FRAME_BYTES};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, UnixListener},
};

#[tokio::test]
async fn relays_exactly_one_bounded_json_frame_without_reflecting_secrets() {
    let fixture = RelayFixture::start().await;
    let request = br#"{"token":"proxy-token-canary","requestId":"request-1","method":"GET","url":"https://service.virtual.invalid/account","headers":{},"bodyBase64":null,"cookieJarSessionId":null,"credentialScope":null}"#.to_vec();
    let expected = request.clone();
    let backend = fixture.spawn_backend(move |frame| async move {
        assert_eq!(frame, expected);
        br#"{"status":200,"headers":{"content-type":"application/json"},"bodyBase64":"e30=","error":null}"#.to_vec()
    });

    let response = post_json(fixture.address, &request).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body,
        br#"{"status":200,"headers":{"content-type":"application/json"},"bodyBase64":"e30=","error":null}"#
    );
    assert!(!response
        .body
        .windows("proxy-token-canary".len())
        .any(|bytes| { bytes == b"proxy-token-canary" }));
    backend.await.unwrap();
    fixture.shutdown();
}

#[tokio::test]
async fn preserves_the_exact_one_mib_newline_frame_contract() {
    let fixture = RelayFixture::start().await;
    let request = json_frame_of_size(MAX_PROXY_FRAME_BYTES);
    let expected = request.clone();
    let backend = fixture.spawn_backend(move |frame| async move {
        assert_eq!(frame, expected);
        br#"{"status":204,"headers":{},"bodyBase64":null,"error":null}"#.to_vec()
    });

    let accepted = post_json(fixture.address, &request).await;
    assert_eq!(accepted.status, 200);
    backend.await.unwrap();

    let oversized = json_frame_of_size(MAX_PROXY_FRAME_BYTES + 1);
    let rejected = post_json(fixture.address, &oversized).await;
    assert_eq!(rejected.status, 413);
    assert_safe_error(&rejected.body, &oversized);

    let two_frames = b"{}\n{}";
    let rejected = post_json(fixture.address, two_frames).await;
    assert_eq!(rejected.status, 400);
    assert_safe_error(&rejected.body, two_frames);
    fixture.shutdown();
}

#[tokio::test]
async fn rejects_invalid_backend_frames_with_a_safe_standard_error() {
    let fixture = RelayFixture::start().await;
    let request = br#"{"token":"backend-failure-token-canary"}"#.to_vec();
    let backend =
        fixture.spawn_backend(|_| async { b"{\"status\":200}\n{\"second\":true}".to_vec() });

    let response = post_json(fixture.address, &request).await;

    assert_eq!(response.status, 502);
    assert_safe_error(&response.body, &request);
    backend.await.unwrap();
    fixture.shutdown();
}

#[tokio::test]
async fn rejects_slow_http_bodies_and_times_out_the_whole_request() {
    let fixture = RelayFixture::start_with_limits(GatewayLimits {
        body_timeout: std::time::Duration::from_millis(40),
        server_timeout: std::time::Duration::from_millis(120),
        max_concurrency: 4,
    })
    .await;

    let mut stream = TcpStream::connect(fixture.address).await.unwrap();
    stream
        .write_all(
            b"POST / HTTP/1.1\r\nHost: gateway\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{",
        )
        .await
        .unwrap();
    let response = read_http_response(stream).await;
    assert_eq!(response.status, 408);

    let backend = fixture.spawn_backend(|_| async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        br#"{"status":200,"headers":{},"bodyBase64":null,"error":null}"#.to_vec()
    });
    let response = post_json(fixture.address, b"{}").await;
    assert_eq!(response.status, 504);
    backend.await.unwrap();
    fixture.shutdown();
}

#[tokio::test]
async fn rejects_requests_beyond_the_concurrency_limit() {
    let fixture = RelayFixture::start_with_limits(GatewayLimits {
        body_timeout: std::time::Duration::from_secs(1),
        server_timeout: std::time::Duration::from_secs(1),
        max_concurrency: 1,
    })
    .await;
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let entered_backend = entered.clone();
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_backend = release.clone();
    let backend = fixture.spawn_backend(move |_| async move {
        entered_backend.notify_one();
        release_backend.notified().await;
        br#"{"status":200,"headers":{},"bodyBase64":null,"error":null}"#.to_vec()
    });
    let address = fixture.address;
    let first = tokio::spawn(async move { post_json(address, b"{}").await });
    entered.notified().await;

    let rejected = post_json(fixture.address, b"{}").await;
    assert_eq!(rejected.status, 503);
    release.notify_one();
    assert_eq!(first.await.unwrap().status, 200);
    backend.await.unwrap();
    fixture.shutdown();
}

#[tokio::test]
async fn rejects_delayed_bytes_after_the_backend_newline() {
    let fixture = RelayFixture::start().await;
    let path = fixture.backend_socket.clone();
    let backend = tokio::spawn(async move {
        let listener = UnixListener::bind(path).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut framed = Vec::new();
        BufReader::new(reader)
            .read_until(b'\n', &mut framed)
            .await
            .unwrap();
        writer
            .write_all(b"{\"status\":200,\"headers\":{},\"bodyBase64\":null,\"error\":null}\n")
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        writer.write_all(b"{\"trailing\":true}").await.unwrap();
        writer.shutdown().await.unwrap();
    });

    let response = post_json(fixture.address, b"{}").await;
    assert_eq!(response.status, 502);
    backend.await.unwrap();
    fixture.shutdown();
}

fn json_frame_of_size(size: usize) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"padding\":\"";
    const SUFFIX: &[u8] = b"\"}";
    assert!(size >= PREFIX.len() + SUFFIX.len());
    let mut frame = Vec::with_capacity(size);
    frame.extend_from_slice(PREFIX);
    frame.resize(size - SUFFIX.len(), b'x');
    frame.extend_from_slice(SUFFIX);
    assert_eq!(frame.len(), size);
    frame
}

fn assert_safe_error(response: &[u8], request: &[u8]) {
    let value: serde_json::Value = serde_json::from_slice(response).unwrap();
    assert!(value["status"].as_u64().is_some());
    assert_eq!(value["headers"], serde_json::json!({}));
    assert!(value["bodyBase64"].is_null());
    assert!(value["error"]["code"].as_str().is_some());
    assert!(value["error"]["summary"].as_str().is_some());
    assert!(!response
        .windows(request.len())
        .any(|bytes| bytes == request));
}

struct RelayFixture {
    _directory: tempfile::TempDir,
    backend_socket: PathBuf,
    address: SocketAddr,
    server: tokio::task::JoinHandle<Result<(), audiodown_proxy_gateway::GatewayError>>,
}

impl RelayFixture {
    async fn start() -> Self {
        Self::start_with_limits(GatewayLimits::default()).await
    }

    async fn start_with_limits(limits: GatewayLimits) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let backend_socket = directory.path().join("core.sock");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_with_limits(listener, backend_socket.clone(), limits));
        Self {
            _directory: directory,
            backend_socket,
            address,
            server,
        }
    }

    fn spawn_backend<F, Fut>(&self, response: F) -> tokio::task::JoinHandle<()>
    where
        F: FnOnce(Vec<u8>) -> Fut + Send + 'static,
        Fut: Future<Output = Vec<u8>> + Send + 'static,
    {
        let path = self.backend_socket.clone();
        tokio::spawn(async move {
            let listener = UnixListener::bind(path).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut framed = Vec::new();
            BufReader::new(reader)
                .read_until(b'\n', &mut framed)
                .await
                .unwrap();
            assert_eq!(framed.pop(), Some(b'\n'));
            let response = response(framed).await;
            if writer.write_all(&response).await.is_err() {
                return;
            }
            if writer.write_all(b"\n").await.is_err() {
                return;
            }
            let _ = writer.shutdown().await;
        })
    }

    fn shutdown(self) {
        self.server.abort();
    }
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

async fn post_json(address: SocketAddr, body: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let head = format!(
        "POST / HTTP/1.1\r\nHost: audiodown-gateway:18081\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    if let Err(error) = stream.write_all(body).await {
        assert!(
            body.len() > MAX_PROXY_FRAME_BYTES
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                )
        );
    }
    read_http_response(stream).await
}

async fn read_http_response(mut stream: TcpStream) -> HttpResponse {
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let separator = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap();
    let head = std::str::from_utf8(&response[..separator]).unwrap();
    let status = head
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    HttpResponse {
        status,
        body: response[separator + 4..].to_vec(),
    }
}
