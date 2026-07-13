use std::{
    collections::VecDeque,
    io::Write,
    net::{IpAddr, Ipv4Addr},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Barrier, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use audiodown_network_proxy::{
    error::HttpProxyError,
    http::{
        HttpProxy, HttpTransport, ProxyRequest, ReqwestTransport, TransportError, TransportRequest,
        TransportResponse,
    },
    policy::{ProxyPolicy, ProxyPolicyError},
    resolver::{DnsResolver, ResolveError, StaticResolver},
};
use audiodown_plugin_api::manifest::PluginManifest;
use flate2::{write::GzEncoder, Compression};
use http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
    task::JoinHandle,
    time::sleep,
};

const HOST: &str = "api.virtual.invalid";
const FIXTURE_PORT: u16 = 18080;
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenTarget {
    url: String,
    host: String,
    port: u16,
    addresses: Vec<IpAddr>,
}

#[derive(Clone, Default)]
struct ScriptedTransport {
    outcomes: Arc<Mutex<VecDeque<Result<TransportResponse, TransportError>>>>,
    seen: Arc<Mutex<Vec<(SeenTarget, TransportRequest)>>>,
}

impl ScriptedTransport {
    fn with_responses(responses: impl IntoIterator<Item = TransportResponse>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(
                responses.into_iter().map(Ok).collect::<VecDeque<_>>(),
            )),
            seen: Arc::default(),
        }
    }

    fn with_outcomes(
        outcomes: impl IntoIterator<Item = Result<TransportResponse, TransportError>>,
    ) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
            seen: Arc::default(),
        }
    }

    fn seen(&self) -> Vec<(SeenTarget, TransportRequest)> {
        self.seen.lock().expect("seen lock").clone()
    }
}

#[async_trait]
impl HttpTransport for ScriptedTransport {
    async fn execute(
        &self,
        target: &audiodown_network_proxy::policy::PinnedTarget,
        request: TransportRequest,
    ) -> Result<TransportResponse, TransportError> {
        self.seen.lock().expect("seen lock").push((
            SeenTarget {
                url: target.url().to_string(),
                host: target.host().to_string(),
                port: target.port(),
                addresses: target.pinned_addresses().to_vec(),
            },
            request,
        ));
        self.outcomes
            .lock()
            .expect("outcome lock")
            .pop_front()
            .expect("complete scripted transport response")
    }
}

#[derive(Clone, Default)]
struct GateTransport {
    entered: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    changed: Arc<Notify>,
}

impl GateTransport {
    async fn wait_for_entries(&self, count: usize) {
        while self.entered.load(Ordering::SeqCst) < count {
            self.changed.notified().await;
        }
    }

    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.changed.notify_waiters();
    }
}

#[async_trait]
impl HttpTransport for GateTransport {
    async fn execute(
        &self,
        _target: &audiodown_network_proxy::policy::PinnedTarget,
        _request: TransportRequest,
    ) -> Result<TransportResponse, TransportError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_waiters();
        while !self.released.load(Ordering::SeqCst) {
            self.changed.notified().await;
        }
        Ok(ok_response(b"released".to_vec()))
    }
}

#[derive(Clone)]
struct DelayedTransport {
    outcomes: Arc<Mutex<VecDeque<(Duration, TransportResponse)>>>,
}

impl DelayedTransport {
    fn new(outcomes: impl IntoIterator<Item = (Duration, TransportResponse)>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
        }
    }
}

#[async_trait]
impl HttpTransport for DelayedTransport {
    async fn execute(
        &self,
        _target: &audiodown_network_proxy::policy::PinnedTarget,
        _request: TransportRequest,
    ) -> Result<TransportResponse, TransportError> {
        let (delay, response) = self
            .outcomes
            .lock()
            .expect("outcome lock")
            .pop_front()
            .expect("complete delayed response");
        sleep(delay).await;
        Ok(response)
    }
}

#[derive(Clone)]
struct YieldObservingTransport {
    body: Arc<Vec<u8>>,
    ticks: Arc<AtomicUsize>,
    observed_before_decode: Arc<AtomicUsize>,
}

#[async_trait]
impl HttpTransport for YieldObservingTransport {
    async fn execute(
        &self,
        _target: &audiodown_network_proxy::policy::PinnedTarget,
        _request: TransportRequest,
    ) -> Result<TransportResponse, TransportError> {
        self.observed_before_decode
            .store(self.ticks.load(Ordering::SeqCst), Ordering::SeqCst);
        Ok(encoded_response("gzip", self.body.as_ref().clone()))
    }
}

struct BlockingResolver {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
    notified: Arc<Notify>,
    calls: usize,
}

impl DnsResolver for BlockingResolver {
    fn resolve(&mut self, _host: &str) -> Result<Vec<IpAddr>, ResolveError> {
        if self.calls > 0 {
            self.notified.notify_one();
            self.entered.wait();
            self.release.wait();
        }
        self.calls += 1;
        Ok(vec![public_v4(10)])
    }
}

#[tokio::test]
async fn sends_the_complete_request_to_the_exact_pinned_target_and_filters_the_response() {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/test"));
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_static("secret=hidden"),
    );
    headers.insert("x-upstream-private", HeaderValue::from_static("hidden"));
    let transport = ScriptedTransport::with_responses([TransportResponse {
        status: StatusCode::PARTIAL_CONTENT,
        headers,
        body: b"payload".to_vec(),
    }]);
    let proxy = fixture_proxy(transport.clone());
    let mut request = request(Method::POST, "/start?token=request-secret");
    request
        .headers
        .insert(header::ACCEPT, HeaderValue::from_static("audio/*"));
    request
        .headers
        .insert("x-client", HeaderValue::from_static("value"));
    request.body = b"request-body".to_vec();

    let response = proxy.execute(request).await.expect("proxy response");

    assert_eq!(response.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.body, b"payload");
    assert_eq!(response.headers.len(), 1);
    assert_eq!(response.headers[header::CONTENT_TYPE], "audio/test");
    let seen = transport.seen();
    assert_eq!(seen[0].0.host, HOST);
    assert_eq!(seen[0].0.port, FIXTURE_PORT);
    assert_eq!(seen[0].0.addresses, [IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    assert_eq!(seen[0].1.method, Method::POST);
    assert_eq!(seen[0].1.url, seen[0].0.url);
    assert_eq!(seen[0].1.headers[header::ACCEPT], "audio/*");
    assert_eq!(seen[0].1.headers["x-client"], "value");
    assert_eq!(seen[0].1.body, b"request-body");
}

#[tokio::test]
async fn method_allowlist_matches_the_node_sdk_contract() {
    let allowed = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];
    let transport = ScriptedTransport::with_responses(
        allowed
            .iter()
            .map(|method| ok_response(method.as_bytes().to_vec())),
    );
    let proxy = fixture_proxy(transport);

    for method in allowed {
        let method = Method::from_bytes(method.as_bytes()).expect("method");
        assert!(proxy.execute(request(method, "/method")).await.is_ok());
    }

    for method in [Method::OPTIONS, Method::CONNECT, Method::TRACE] {
        assert_eq!(
            proxy.execute(request(method, "/method")).await,
            Err(HttpProxyError::MethodNotAllowed)
        );
    }
}

#[tokio::test]
async fn structurally_rejects_sensitive_forwarding_and_framing_request_headers() {
    for name in [
        "host",
        "cookie",
        "set-cookie",
        "authorization",
        "proxy-authorization",
        "proxy-connection",
        "forwarded",
        "x-forwarded-for",
        "x-real-ip",
        "connection",
        "keep-alive",
        "transfer-encoding",
        "content-length",
        "expect",
        "te",
        "trailer",
        "upgrade",
    ] {
        let proxy = fixture_proxy(ScriptedTransport::default());
        let mut request = request(Method::GET, "/headers");
        request.headers.insert(
            HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            HeaderValue::from_static("secret"),
        );
        assert_eq!(
            proxy.execute(request).await,
            Err(HttpProxyError::RequestHeadersTooLarge),
            "{name} should be rejected"
        );
    }
}

#[tokio::test]
async fn enforces_request_header_count_at_the_fixed_boundary() {
    let transport = ScriptedTransport::with_responses([ok_response(vec![])]);
    let proxy = fixture_proxy(transport);
    let mut at_limit = request(Method::GET, "/headers");
    for index in 0..32 {
        at_limit.headers.insert(
            HeaderName::from_bytes(format!("x-field-{index}").as_bytes()).unwrap(),
            HeaderValue::from_static("v"),
        );
    }
    assert!(proxy.execute(at_limit).await.is_ok());

    let mut over_limit = request(Method::GET, "/headers");
    for index in 0..33 {
        over_limit.headers.insert(
            HeaderName::from_bytes(format!("x-field-{index}").as_bytes()).unwrap(),
            HeaderValue::from_static("v"),
        );
    }
    assert_eq!(
        proxy.execute(over_limit).await,
        Err(HttpProxyError::RequestHeadersTooLarge)
    );
}

#[tokio::test]
async fn enforces_request_header_bytes_at_the_fixed_boundary() {
    let transport = ScriptedTransport::with_responses([ok_response(vec![])]);
    let proxy = fixture_proxy(transport);
    let mut at_limit = request(Method::GET, "/headers");
    at_limit.headers.insert(
        "x-pad",
        HeaderValue::from_bytes(&vec![b'a'; 16 * 1024 - 5]).unwrap(),
    );
    assert!(proxy.execute(at_limit).await.is_ok());

    let mut over_limit = request(Method::GET, "/headers");
    over_limit.headers.insert(
        "x-pad",
        HeaderValue::from_bytes(&vec![b'a'; 16 * 1024 - 4]).unwrap(),
    );
    assert_eq!(
        proxy.execute(over_limit).await,
        Err(HttpProxyError::RequestHeadersTooLarge)
    );
}

#[tokio::test]
async fn enforces_request_body_bytes_at_the_fixed_boundary() {
    let transport = ScriptedTransport::with_responses([ok_response(vec![])]);
    let proxy = fixture_proxy(transport);
    let mut at_limit = request(Method::POST, "/upload");
    at_limit.body = vec![b'a'; MAX_REQUEST_BODY_BYTES];
    assert!(proxy.execute(at_limit).await.is_ok());

    let mut over_limit = request(Method::POST, "/upload");
    over_limit.body = vec![b'a'; MAX_REQUEST_BODY_BYTES + 1];
    assert_eq!(
        proxy.execute(over_limit).await,
        Err(HttpProxyError::RequestBodyTooLarge)
    );
}

#[tokio::test]
async fn resolves_relative_redirects_and_reauthorizes_each_hop() {
    let first = redirect_response(StatusCode::FOUND, "/next?part=2");
    let transport = ScriptedTransport::with_responses([first, ok_response(b"done".to_vec())]);
    let proxy = fixture_proxy(transport.clone());

    let response = proxy
        .execute(request(Method::GET, "/start"))
        .await
        .expect("redirected response");

    assert_eq!(response.body, b"done");
    let seen = transport.seen();
    assert_eq!(seen[0].0.url, fixture_url("/start"));
    assert_eq!(seen[1].0.url, fixture_url("/next?part=2"));
}

#[tokio::test]
async fn rejects_dns_rebinding_during_redirect_reauthorization() {
    let resolver = SequenceResolver::new([public_v4(10), public_v4(11)]);
    let policy = ProxyPolicy::production(&manifest([HOST]));
    let transport = ScriptedTransport::with_responses([redirect_response(
        StatusCode::TEMPORARY_REDIRECT,
        "/next",
    )]);
    let proxy = HttpProxy::new(policy, resolver, transport);

    assert_eq!(
        proxy
            .execute(ProxyRequest {
                method: Method::GET,
                url: format!("https://{HOST}/start"),
                headers: HeaderMap::new(),
                body: vec![],
            })
            .await,
        Err(HttpProxyError::Policy(ProxyPolicyError::DnsRebinding))
    );
}

#[tokio::test]
async fn rejects_redirect_host_changes_as_policy_errors() {
    let other = "other.virtual.invalid";
    let resolver = StaticResolver::new([(HOST, vec![public_v4(10)]), (other, vec![public_v4(10)])]);
    let policy = ProxyPolicy::production(&manifest([HOST, other]));
    let transport = ScriptedTransport::with_responses([redirect_response(
        StatusCode::MOVED_PERMANENTLY,
        &format!("https://{other}/next"),
    )]);
    let proxy = HttpProxy::new(policy, resolver, transport);

    assert_eq!(
        proxy
            .execute(ProxyRequest {
                method: Method::GET,
                url: format!("https://{HOST}/start"),
                headers: HeaderMap::new(),
                body: vec![],
            })
            .await,
        Err(HttpProxyError::Policy(
            ProxyPolicyError::RedirectHostChanged
        ))
    );
}

#[tokio::test]
async fn bounds_redirect_chains_by_followed_hops() {
    let transport = ScriptedTransport::with_responses(
        (0..=5).map(|_| redirect_response(StatusCode::FOUND, "/loop")),
    );
    let proxy = fixture_proxy(transport);

    assert_eq!(
        proxy.execute(request(Method::GET, "/loop")).await,
        Err(HttpProxyError::TooManyRedirects)
    );
}

#[tokio::test]
async fn rejects_missing_malformed_and_unsafe_redirect_locations() {
    let mut missing = empty_response(StatusCode::FOUND);
    missing.headers = HeaderMap::new();
    let mut non_utf8 = empty_response(StatusCode::FOUND);
    non_utf8.headers.insert(
        header::LOCATION,
        HeaderValue::from_bytes(b"/next\xff").unwrap(),
    );
    let unsafe_scheme = redirect_response(StatusCode::FOUND, "file:///tmp/item");
    let mut duplicate = redirect_response(StatusCode::FOUND, "/one");
    duplicate
        .headers
        .append(header::LOCATION, HeaderValue::from_static("/two"));

    for response in [missing, non_utf8, unsafe_scheme, duplicate] {
        let proxy = fixture_proxy(ScriptedTransport::with_responses([response]));
        assert_eq!(
            proxy.execute(request(Method::GET, "/redirect")).await,
            Err(HttpProxyError::InvalidRedirect)
        );
    }
}

#[tokio::test]
async fn preserves_methods_for_307_and_rejects_unsafe_302_upload_redirects() {
    let transport = ScriptedTransport::with_responses([
        redirect_response(StatusCode::TEMPORARY_REDIRECT, "/continued"),
        ok_response(vec![]),
    ]);
    let proxy = fixture_proxy(transport.clone());
    let mut upload = request(Method::PUT, "/upload");
    upload.body = b"payload".to_vec();
    assert!(proxy.execute(upload).await.is_ok());
    let seen = transport.seen();
    assert_eq!(seen[1].1.method, Method::PUT);
    assert_eq!(seen[1].1.body, b"payload");

    let proxy = fixture_proxy(ScriptedTransport::with_responses([redirect_response(
        StatusCode::FOUND,
        "/unsafe",
    )]));
    assert_eq!(
        proxy.execute(request(Method::POST, "/upload")).await,
        Err(HttpProxyError::InvalidRedirect)
    );
}

#[tokio::test]
async fn counts_response_headers_before_filtering_at_both_boundaries() {
    let at_limit = response_with_headers(64, 1);
    let over_limit = response_with_headers(65, 1);
    let proxy = fixture_proxy(ScriptedTransport::with_responses([at_limit, over_limit]));
    assert!(proxy
        .execute(request(Method::GET, "/response"))
        .await
        .is_ok());
    assert_eq!(
        proxy.execute(request(Method::GET, "/response")).await,
        Err(HttpProxyError::ResponseHeadersTooLarge)
    );
}

#[tokio::test]
async fn counts_response_header_bytes_before_filtering_at_both_boundaries() {
    let at_limit = response_with_headers(1, 32 * 1024 - 9);
    let over_limit = response_with_headers(1, 32 * 1024 - 8);
    let proxy = fixture_proxy(ScriptedTransport::with_responses([at_limit, over_limit]));
    assert!(proxy
        .execute(request(Method::GET, "/response"))
        .await
        .is_ok());
    assert_eq!(
        proxy.execute(request(Method::GET, "/response")).await,
        Err(HttpProxyError::ResponseHeadersTooLarge)
    );
}

#[tokio::test]
async fn rejects_invalid_duplicate_and_oversized_response_framing() {
    let mut malformed = ok_response(vec![]);
    malformed
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("unknown"));
    let mut duplicate = ok_response(vec![]);
    duplicate
        .headers
        .append(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    duplicate
        .headers
        .append(header::CONTENT_LENGTH, HeaderValue::from_static("1"));
    let mut conflicting = ok_response(vec![]);
    conflicting
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    conflicting.headers.insert(
        header::TRANSFER_ENCODING,
        HeaderValue::from_static("chunked"),
    );

    for response in [malformed, duplicate, conflicting] {
        let proxy = fixture_proxy(ScriptedTransport::with_responses([response]));
        assert_eq!(
            proxy.execute(request(Method::GET, "/framing")).await,
            Err(HttpProxyError::Transport)
        );
    }

    let mut oversized = ok_response(vec![]);
    oversized.headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&(MAX_RESPONSE_BODY_BYTES + 1).to_string()).unwrap(),
    );
    let proxy = fixture_proxy(ScriptedTransport::with_responses([oversized]));
    assert_eq!(
        proxy.execute(request(Method::GET, "/framing")).await,
        Err(HttpProxyError::ResponseBodyTooLarge)
    );
}

#[tokio::test]
async fn bounds_identity_response_bytes_without_trusting_content_length() {
    let at_limit = ok_response(vec![b'a'; MAX_RESPONSE_BODY_BYTES]);
    let over_limit = ok_response(vec![b'a'; MAX_RESPONSE_BODY_BYTES + 1]);
    let proxy = fixture_proxy(ScriptedTransport::with_responses([at_limit, over_limit]));
    assert_eq!(
        proxy
            .execute(request(Method::GET, "/large"))
            .await
            .expect("at limit")
            .body
            .len(),
        MAX_RESPONSE_BODY_BYTES
    );
    assert_eq!(
        proxy.execute(request(Method::GET, "/large")).await,
        Err(HttpProxyError::ResponseBodyTooLarge)
    );

    let mut understated = ok_response(vec![b'a'; MAX_RESPONSE_BODY_BYTES + 1]);
    understated
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("1"));
    let proxy = fixture_proxy(ScriptedTransport::with_responses([understated]));
    assert_eq!(
        proxy.execute(request(Method::GET, "/large")).await,
        Err(HttpProxyError::ResponseBodyTooLarge)
    );
}

#[tokio::test]
async fn bounds_the_decoded_gzip_body_and_removes_encoding_metadata() {
    let decoded = vec![b'z'; 1024 * 1024];
    let proxy = fixture_proxy(ScriptedTransport::with_responses([encoded_response(
        "gzip",
        gzip(&decoded),
    )]));
    let response = proxy
        .execute(request(Method::GET, "/compressed"))
        .await
        .expect("gzip response");
    assert_eq!(response.body, decoded);
    assert!(!response.headers.contains_key(header::CONTENT_ENCODING));
    assert!(!response.headers.contains_key(header::CONTENT_LENGTH));

    let expanded = vec![b'z'; MAX_RESPONSE_BODY_BYTES + 1];
    let proxy = fixture_proxy(ScriptedTransport::with_responses([encoded_response(
        "gzip",
        gzip(&expanded),
    )]));
    assert_eq!(
        proxy.execute(request(Method::GET, "/compressed")).await,
        Err(HttpProxyError::ResponseBodyTooLarge)
    );
}

#[tokio::test]
async fn rejects_unsupported_stacked_malformed_and_corrupt_response_encodings() {
    let mut non_utf8 = ok_response(vec![]);
    non_utf8.headers.insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_bytes(b"gzip\xff").unwrap(),
    );
    for response in [
        encoded_response("br", vec![]),
        encoded_response("gzip, identity", vec![]),
        non_utf8,
        encoded_response("gzip", b"not-gzip".to_vec()),
    ] {
        let proxy = fixture_proxy(ScriptedTransport::with_responses([response]));
        assert_eq!(
            proxy.execute(request(Method::GET, "/encoding")).await,
            Err(HttpProxyError::InvalidResponseEncoding)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn applies_the_fixed_timeout_to_injected_transports() {
    let proxy = fixture_proxy(GateTransport::default());
    assert_eq!(
        proxy.execute(request(Method::GET, "/slow")).await,
        Err(HttpProxyError::Timeout)
    );
}

#[tokio::test]
async fn rejects_saturation_instead_of_queueing_untrusted_work() {
    let transport = GateTransport::default();
    let proxy = Arc::new(fixture_proxy(transport.clone()));
    let tasks = spawn_requests(proxy.clone(), 8);
    transport.wait_for_entries(8).await;

    assert_eq!(
        proxy.execute(request(Method::GET, "/ninth")).await,
        Err(HttpProxyError::ConcurrencyLimited)
    );

    transport.release();
    join_all(tasks).await;
}

#[tokio::test]
async fn dropping_an_inflight_request_releases_its_concurrency_permit() {
    let transport = GateTransport::default();
    let proxy = Arc::new(fixture_proxy(transport.clone()));
    let pending = {
        let proxy = proxy.clone();
        tokio::spawn(async move { proxy.execute(request(Method::GET, "/cancel")).await })
    };
    transport.wait_for_entries(1).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());

    transport.release();
    let tasks = spawn_requests(proxy, 8);
    join_all(tasks).await;
}

#[tokio::test]
async fn maps_transport_failures_to_a_stable_non_sensitive_error() {
    let transport = ScriptedTransport::with_outcomes([Err(TransportError)]);
    let proxy = fixture_proxy(transport);
    let error = proxy
        .execute(request(Method::GET, "/failure?credential=must-not-leak"))
        .await
        .expect_err("transport error");
    assert_eq!(error, HttpProxyError::Transport);
    let display = error.to_string();
    assert_eq!(display, "proxy transport failed");
    assert!(!display.contains("credential"));
    assert!(!display.contains(HOST));
}

#[tokio::test]
async fn reqwest_transport_connects_to_the_pin_while_preserving_original_host() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fixture");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let count = socket.read(&mut chunk).await.expect("read request");
            assert!(count > 0, "request headers should arrive");
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nSet-Cookie: secret=hidden\r\nConnection: close\r\n\r\nok",
            )
            .await
            .expect("write response");
        String::from_utf8(bytes).expect("HTTP request text")
    });
    let policy = ProxyPolicy::developer(&manifest([HOST]))
        .with_fixture_mapping(HOST, IpAddr::V4(Ipv4Addr::LOCALHOST))
        .expect("fixture mapping");
    let proxy = HttpProxy::new(policy, StaticResolver::empty(), ReqwestTransport);

    let response = proxy
        .execute(ProxyRequest {
            method: Method::GET,
            url: format!("http://{HOST}:{port}/media?q=1"),
            headers: HeaderMap::new(),
            body: vec![],
        })
        .await
        .expect("real transport response");

    assert_eq!(response.body, b"ok");
    assert!(!response.headers.contains_key(header::SET_COOKIE));
    let request = server.await.expect("server task");
    assert!(request.starts_with("GET /media?q=1 HTTP/1.1\r\n"));
    assert!(request
        .to_ascii_lowercase()
        .contains(&format!("host: {HOST}:{port}\r\n")));
}

#[tokio::test]
async fn reqwest_transport_pins_a_trailing_dot_authority_without_ambient_dns() {
    let (port, server) =
        serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec())
            .await;
    let policy = ProxyPolicy::developer(&manifest([HOST]))
        .with_fixture_mapping(HOST, IpAddr::V4(Ipv4Addr::LOCALHOST))
        .expect("fixture mapping");
    let proxy = HttpProxy::new(policy, StaticResolver::empty(), ReqwestTransport);

    let response = proxy
        .execute(ProxyRequest {
            method: Method::GET,
            url: format!("http://{HOST}.:{port}/trailing-dot"),
            headers: HeaderMap::new(),
            body: vec![],
        })
        .await
        .expect("trailing-dot authority must use its explicit pin");

    assert_eq!(response.body, b"ok");
    let request = server.await.expect("server task");
    assert!(request
        .to_ascii_lowercase()
        .contains(&format!("host: {HOST}.:{port}\r\n")));
}

#[tokio::test(start_paused = true)]
async fn one_operation_deadline_covers_every_redirect_hop() {
    let transport = DelayedTransport::new([
        (
            Duration::from_secs(20),
            redirect_response(StatusCode::TEMPORARY_REDIRECT, "/second"),
        ),
        (Duration::from_secs(20), ok_response(b"too-late".to_vec())),
    ]);
    let proxy = fixture_proxy(transport);

    assert_eq!(
        proxy.execute(request(Method::GET, "/first")).await,
        Err(HttpProxyError::Timeout)
    );
}

#[tokio::test(start_paused = true)]
async fn redirect_authorization_resolver_work_is_governed_by_the_operation_deadline() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let notified = Arc::new(Notify::new());
    let resolver = BlockingResolver {
        entered: entered.clone(),
        release: release.clone(),
        notified: notified.clone(),
        calls: 0,
    };
    let helper = std::thread::spawn(move || {
        entered.wait();
        std::thread::sleep(Duration::from_millis(100));
        release.wait();
    });
    let policy = ProxyPolicy::production(&manifest([HOST]));
    let proxy = Arc::new(HttpProxy::new(
        policy,
        resolver,
        ScriptedTransport::with_responses([
            redirect_response(StatusCode::TEMPORARY_REDIRECT, "/redirected"),
            ok_response(vec![]),
        ]),
    ));

    let request = tokio::spawn(async move {
        proxy
            .execute(ProxyRequest {
                method: Method::GET,
                url: format!("https://{HOST}/slow-resolution"),
                headers: HeaderMap::new(),
                body: vec![],
            })
            .await
    });
    notified.notified().await;
    tokio::time::advance(Duration::from_secs(31)).await;
    let result = request.await.expect("proxy task");
    helper.join().expect("resolver release thread");

    assert_eq!(result, Err(HttpProxyError::Timeout));
}

#[tokio::test]
async fn bounded_gzip_decoding_yields_the_async_runtime() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let observed_before_decode = Arc::new(AtomicUsize::new(usize::MAX));
    let done = Arc::new(AtomicBool::new(false));
    let transport = YieldObservingTransport {
        body: Arc::new(gzip(&vec![b'z'; 8 * 1024 * 1024])),
        ticks: ticks.clone(),
        observed_before_decode: observed_before_decode.clone(),
    };
    let proxy = fixture_proxy(transport);
    let ticker = {
        let ticks = ticks.clone();
        let done = done.clone();
        tokio::spawn(async move {
            while !done.load(Ordering::SeqCst) {
                ticks.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        })
    };

    let response = proxy
        .execute(request(Method::GET, "/compressed"))
        .await
        .expect("gzip response");
    let runtime_progressed =
        ticks.load(Ordering::SeqCst) > observed_before_decode.load(Ordering::SeqCst);
    done.store(true, Ordering::SeqCst);
    ticker.await.expect("ticker task");

    assert_eq!(response.body.len(), 8 * 1024 * 1024);
    assert!(runtime_progressed, "gzip decode blocked the async worker");
}

#[tokio::test]
async fn reqwest_transport_rejects_an_oversized_wire_header_safely() {
    let mut response = b"HTTP/1.1 200 OK\r\nX-Oversized: ".to_vec();
    response.extend(vec![b'a'; 512 * 1024]);
    response.extend_from_slice(b"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let (port, server) = serve_once(response).await;
    let proxy = wire_proxy(port);

    assert!(matches!(
        proxy.execute(wire_request(port, "/oversized-header")).await,
        Err(HttpProxyError::Transport | HttpProxyError::ResponseHeadersTooLarge)
    ));
    let _ = server.await.expect("server task");
}

#[tokio::test]
async fn hyper_http1_parser_rejects_more_than_its_builtin_wire_header_count() {
    let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
    for index in 0..101 {
        response.extend_from_slice(format!("X-Wire-{index}: v\r\n").as_bytes());
    }
    response.extend_from_slice(b"Content-Length: 0\r\nConnection: close\r\n\r\n");
    let (port, server) = serve_once(response).await;
    let proxy = wire_proxy(port);

    assert_eq!(
        proxy.execute(wire_request(port, "/header-count")).await,
        Err(HttpProxyError::Transport)
    );
    let _ = server.await.expect("server task");
}

#[tokio::test]
async fn reqwest_transport_manually_redirects_then_decodes_and_filters_wire_response() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fixture");
    let port = listener.local_addr().unwrap().port();
    let compressed = gzip(b"wire-gzip");
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        let (mut first, _) = listener.accept().await.expect("first request");
        requests.push(read_wire_request(&mut first).await);
        first
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nSet-Cookie: redirect=hidden\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("first response");

        let (mut second, _) = listener.accept().await.expect("second request");
        requests.push(read_wire_request(&mut second).await);
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/test\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nSet-Cookie: final=hidden\r\nConnection: close\r\n\r\n",
            compressed.len()
        );
        second
            .write_all(headers.as_bytes())
            .await
            .expect("second headers");
        second.write_all(&compressed).await.expect("second body");
        requests
    });
    let proxy = wire_proxy(port);

    let response = proxy
        .execute(wire_request(port, "/start"))
        .await
        .expect("redirected gzip response");

    assert_eq!(response.body, b"wire-gzip");
    assert_eq!(response.headers[header::CONTENT_TYPE], "audio/test");
    assert!(!response.headers.contains_key(header::SET_COOKIE));
    assert!(!response.headers.contains_key(header::CONTENT_ENCODING));
    assert!(!response.headers.contains_key(header::CONTENT_LENGTH));
    let requests = server.await.expect("server task");
    assert!(requests[0].starts_with("GET /start HTTP/1.1\r\n"));
    assert!(requests[1].starts_with("GET /final HTTP/1.1\r\n"));
}

#[tokio::test]
async fn reqwest_transport_rejects_conflicting_wire_framing() {
    let (port, server) = serve_once(
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\nx\r\n0\r\n\r\n"
            .to_vec(),
    )
    .await;
    let proxy = wire_proxy(port);

    assert_eq!(
        proxy.execute(wire_request(port, "/framing")).await,
        Err(HttpProxyError::Transport)
    );
    let _ = server.await.expect("server task");
}

fn fixture_proxy<T: HttpTransport>(transport: T) -> HttpProxy<StaticResolver, T> {
    let policy = ProxyPolicy::developer(&manifest([HOST]))
        .with_fixture_mapping(HOST, IpAddr::V4(Ipv4Addr::LOCALHOST))
        .expect("fixture mapping");
    HttpProxy::new(policy, StaticResolver::empty(), transport)
}

fn wire_proxy(_port: u16) -> HttpProxy<StaticResolver, ReqwestTransport> {
    let policy = ProxyPolicy::developer(&manifest([HOST]))
        .with_fixture_mapping(HOST, IpAddr::V4(Ipv4Addr::LOCALHOST))
        .expect("fixture mapping");
    HttpProxy::new(policy, StaticResolver::empty(), ReqwestTransport)
}

fn wire_request(port: u16, path: &str) -> ProxyRequest {
    ProxyRequest {
        method: Method::GET,
        url: format!("http://{HOST}:{port}{path}"),
        headers: HeaderMap::new(),
        body: vec![],
    }
}

async fn serve_once(response: Vec<u8>) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fixture");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        let request = read_wire_request(&mut socket).await;
        let _ = socket.write_all(&response).await;
        request
    });
    (port, server)
}

async fn read_wire_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = socket.read(&mut chunk).await.expect("read request");
        assert!(count > 0, "request headers should arrive");
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).expect("HTTP request text")
}

fn request(method: Method, path: &str) -> ProxyRequest {
    ProxyRequest {
        method,
        url: fixture_url(path),
        headers: HeaderMap::new(),
        body: vec![],
    }
}

fn fixture_url(path: &str) -> String {
    format!("http://{HOST}:{FIXTURE_PORT}{path}")
}

fn empty_response(status: StatusCode) -> TransportResponse {
    TransportResponse {
        status,
        headers: HeaderMap::new(),
        body: vec![],
    }
}

fn ok_response(body: Vec<u8>) -> TransportResponse {
    TransportResponse {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        body,
    }
}

fn redirect_response(status: StatusCode, location: &str) -> TransportResponse {
    let mut response = empty_response(status);
    response.headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("location"),
    );
    response
}

fn encoded_response(encoding: &'static str, body: Vec<u8>) -> TransportResponse {
    let mut response = ok_response(body);
    response
        .headers
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
    response
}

fn response_with_headers(count: usize, value_bytes: usize) -> TransportResponse {
    let mut response = ok_response(vec![]);
    for index in 0..count {
        let name = format!("x-field-{index}");
        let value = vec![b'a'; value_bytes];
        response.headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_bytes(&value).unwrap(),
        );
    }
    response
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("compress fixture");
    encoder.finish().expect("finish fixture")
}

fn spawn_requests<T: HttpTransport>(
    proxy: Arc<HttpProxy<StaticResolver, T>>,
    count: usize,
) -> Vec<JoinHandle<Result<audiodown_network_proxy::http::ProxyResponse, HttpProxyError>>> {
    (0..count)
        .map(|index| {
            let proxy = proxy.clone();
            tokio::spawn(async move {
                proxy
                    .execute(request(Method::GET, &format!("/request-{index}")))
                    .await
            })
        })
        .collect()
}

async fn join_all(
    tasks: Vec<JoinHandle<Result<audiodown_network_proxy::http::ProxyResponse, HttpProxyError>>>,
) {
    for task in tasks {
        assert!(task.await.expect("request task").is_ok());
    }
}

struct SequenceResolver {
    answers: VecDeque<IpAddr>,
}

impl SequenceResolver {
    fn new<const N: usize>(answers: [IpAddr; N]) -> Self {
        Self {
            answers: answers.into(),
        }
    }
}

impl DnsResolver for SequenceResolver {
    fn resolve(&mut self, _host: &str) -> Result<Vec<IpAddr>, ResolveError> {
        self.answers
            .pop_front()
            .map(|address| vec![address])
            .ok_or(ResolveError::NotFound)
    }
}

fn manifest<const N: usize>(allowed_hosts: [&str; N]) -> PluginManifest {
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
        "network": {"allowedHosts": allowed_hosts.to_vec()}
    }))
    .expect("valid plugin manifest")
}

fn public_v4(last_octet: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(93, 184, 216, last_octet))
}
