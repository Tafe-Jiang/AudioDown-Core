use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::VecDeque,
    io,
    net::{IpAddr, Ipv4Addr},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use audiodown_credential_vault::{
    CredentialCreateRequest, CredentialRepository, CredentialRepositoryError,
    CredentialUpdateRequest, CredentialVault, MasterKey, TokenCredentialSecret,
};
use audiodown_domain::{
    credential::{CredentialScope, CredentialStatus},
    plugin::{PluginId, PluginStatus, RunMode},
};
use audiodown_network_proxy::{
    cookie_jar::CookieJarSessionId,
    credential::{CredentialGrantPort, CredentialVaultPort},
    http::{HttpTransport, TransportError, TransportRequest, TransportResponse},
    policy::PinnedTarget,
    resolver::StaticResolver,
};
use audiodown_plugin_api::manifest::{CredentialTargetOrigin, PluginManifest, PluginType};
use audiodown_server::{
    app::build_router,
    proxy_adapters::{
        SqliteCoreProxyBackend, SqliteCredentialGrantPort, SqliteCredentialSelector,
        SqliteCredentialVaultPort, SqliteInstalledPluginLoader, SqliteVaultRepository,
    },
    proxy_gateway::{
        AuthenticatedRuntime, CoreProxyBackend, CoreProxyBackendError, CoreProxyRequest,
        CoreProxyResponse, ProxyGateway, ProxyGatewayLimits, ProxyTokenRegistry,
        MAX_PROXY_FRAME_BYTES,
    },
    state::{AppState, UnavailableSupervisorClient},
};
use audiodown_storage::{CredentialScopeGrantRecord, PluginRecord, Storage};
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use secrecy::{Secret, SecretString};
use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::{watch, Notify},
    task::JoinHandle,
    time::timeout,
};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

const CONTENT_PLUGIN: &str = "com.example.proxy-fixture.content";
const CREDENTIAL_PLUGIN: &str = "com.example.proxy-fixture.credential";
const HOST: &str = "api.proxy-fixture.invalid";
const ORIGIN: &str = "https://api.proxy-fixture.invalid";
const SCOPE: &str = "proxyfixture.web";
const TOKEN_CANARY: &str = "task-12-token-canary-must-remain-secret";

struct TrackingAllocator;

thread_local! {
    static TRACKED_ALLOCATION_THRESHOLD: Cell<Option<usize>> = const { Cell::new(None) };
    static SAW_TRACKED_ALLOCATION: Cell<bool> = const { Cell::new(false) };
}

fn record_allocation(size: usize) {
    let _ = TRACKED_ALLOCATION_THRESHOLD.try_with(|threshold| {
        if threshold.get().is_some_and(|threshold| size >= threshold) {
            let _ = SAW_TRACKED_ALLOCATION.try_with(|seen| seen.set(true));
        }
    });
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation(new_size);
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator;

struct AllocationTracker;

impl AllocationTracker {
    fn start(threshold: usize) -> Self {
        SAW_TRACKED_ALLOCATION.with(|seen| seen.set(false));
        TRACKED_ALLOCATION_THRESHOLD.with(|current| current.set(Some(threshold)));
        Self
    }

    fn finish(self) -> bool {
        TRACKED_ALLOCATION_THRESHOLD.with(|current| current.set(None));
        let seen = SAW_TRACKED_ALLOCATION.with(Cell::get);
        std::mem::forget(self);
        seen
    }
}

impl Drop for AllocationTracker {
    fn drop(&mut self) {
        let _ = TRACKED_ALLOCATION_THRESHOLD.try_with(|current| current.set(None));
    }
}

#[test]
fn registry_generates_256_bit_tokens_and_binds_current_runtime_generation() {
    let registry = ProxyTokenRegistry::new();
    let plugin_a = plugin_id(CONTENT_PLUGIN);
    let plugin_b = plugin_id("com.example.proxy-fixture.other");

    let first = registry.register(plugin_a.clone()).expect("register A1");
    let first_token = first.token().with_value(str::to_owned);
    assert_eq!(URL_SAFE_NO_PAD.decode(&first_token).unwrap().len(), 32);
    assert_eq!(
        registry.authenticate(&first_token).unwrap().plugin_id(),
        &plugin_a
    );

    let second = registry.register(plugin_a.clone()).expect("register A2");
    let second_token = second.token().with_value(str::to_owned);
    assert_ne!(first_token, second_token);
    assert!(
        registry.authenticate(&first_token).is_err(),
        "old token must be stale"
    );
    assert_eq!(
        registry.authenticate(&second_token).unwrap().generation(),
        second.generation()
    );

    assert!(!registry.revoke(&plugin_a, first.generation()));
    assert!(registry.authenticate(&second_token).is_ok());
    assert_eq!(
        registry
            .authenticate(
                &registry
                    .register(plugin_b.clone())
                    .unwrap()
                    .token()
                    .with_value(str::to_owned)
            )
            .unwrap()
            .plugin_id(),
        &plugin_b
    );
    assert!(registry.revoke(&plugin_a, second.generation()));
    assert!(registry.authenticate(&second_token).is_err());

    let third = registry.register(plugin_a.clone()).unwrap();
    let third_token = third.token().with_value(str::to_owned);
    assert!(registry.revoke_plugin(&plugin_a));
    assert!(registry.authenticate(&third_token).is_err());
    registry.revoke_all();
    assert_eq!(registry.len(), 0);
}

#[test]
fn registry_is_bounded_concurrent_and_never_renders_or_logs_token_plaintext() {
    let registry = Arc::new(ProxyTokenRegistry::with_capacity(16).unwrap());
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut threads = Vec::new();
    for index in 0..16 {
        let registry = Arc::clone(&registry);
        threads.push(std::thread::spawn(move || {
            let plugin = plugin_id(&format!("com.example.proxy-fixture.concurrent-{index}"));
            let registered = registry.register(plugin.clone()).unwrap();
            let token = registered.token().with_value(str::to_owned);
            assert_eq!(registry.authenticate(&token).unwrap().plugin_id(), &plugin);
            (registered, token)
        }));
    }
    let registrations = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert!(registry
        .register(plugin_id("com.example.proxy-fixture.overflow"))
        .is_err());

    let (removed, _) = &registrations[0];
    assert!(registry.revoke(
        registry
            .authenticate(&registrations[0].1)
            .unwrap()
            .plugin_id(),
        removed.generation(),
    ));
    let logged = registry
        .register(plugin_id("com.example.proxy-fixture.logged"))
        .unwrap();
    let logged_token = logged.token().with_value(str::to_owned);
    assert!(!logs.rendered().contains(&logged_token));

    for (registered, token) in registrations {
        let rendered = format!("{registered:?} {:?}", registered.token());
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains(&token));
        assert!(!logs.rendered().contains(&token));
    }
}

#[test]
fn cookie_jar_wire_identity_requires_canonical_uuid() {
    let canonical = "b57a7b99-c5e4-48eb-bf0b-0769c4a30ca2";
    assert_eq!(
        CookieJarSessionId::parse(canonical).unwrap().to_string(),
        canonical
    );
    for invalid in [
        "B57A7B99-C5E4-48EB-BF0B-0769C4A30CA2",
        "{b57a7b99-c5e4-48eb-bf0b-0769c4a30ca2}",
        "b57a7b99c5e448ebbf0b0769c4a30ca2",
        "not-a-session-id",
    ] {
        assert!(
            CookieJarSessionId::parse(invalid).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn authenticated_request_response_and_jar_debug_are_metadata_only() {
    let jar_value = "b57a7b99-c5e4-48eb-bf0b-0769c4a30ca2";
    let request_body = b"request-debug-body-canary".to_vec();
    let request = CoreProxyRequest {
        request_id: "debug-request-canary".to_string(),
        method: Method::POST,
        url: "https://debug-secret.invalid/private?token=url-canary".to_string(),
        headers: HeaderMap::from_iter([(
            header::AUTHORIZATION,
            HeaderValue::from_static("request-header-canary"),
        )]),
        body: request_body.clone(),
        cookie_jar_session_id: Some(CookieJarSessionId::parse(jar_value).unwrap()),
        credential_scope: None,
    };
    let rendered_request = format!("{request:?}");
    for secret in [
        "debug-request-canary",
        "debug-secret.invalid",
        "url-canary",
        "request-header-canary",
        jar_value,
        &format!("{request_body:?}"),
    ] {
        assert!(
            !rendered_request.contains(secret),
            "request Debug leaked {secret}"
        );
    }
    assert!(rendered_request.contains("header_count"));
    assert!(rendered_request.contains("body_bytes"));

    let response_body = b"response-debug-body-canary".to_vec();
    let response = CoreProxyResponse::new(
        StatusCode::OK,
        HeaderMap::from_iter([(
            header::SET_COOKIE,
            HeaderValue::from_static("response-header-canary"),
        )]),
        response_body.clone(),
    );
    let rendered_response = format!("{response:?}");
    for secret in ["response-header-canary", &format!("{response_body:?}")] {
        assert!(
            !rendered_response.contains(secret),
            "response Debug leaked {secret}"
        );
    }
    assert!(rendered_response.contains("header_count"));
    assert!(rendered_response.contains("body_bytes"));

    let rendered_jar = format!("{:?}", CookieJarSessionId::parse(jar_value).unwrap());
    assert!(rendered_jar.contains("[REDACTED]"));
    assert!(!rendered_jar.contains(jar_value));
}

#[tokio::test]
async fn gateway_authenticates_before_dispatch_and_preserves_sdk_json_shape() {
    let fixture = GatewayFixture::start(FakeBackend::default(), test_limits()).await;
    let plugin = plugin_id(CONTENT_PLUGIN);
    let current = fixture.registry.register(plugin.clone()).unwrap();
    let token = current.token().with_value(str::to_owned);

    let success = fixture.send(request_frame(Some(&token))).await;
    assert_eq!(
        success,
        json!({
            "status": 200,
            "headers": {"content-type": "application/json"},
            "bodyBase64": "e30=",
            "error": null
        })
    );
    let seen = fixture.backend.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0.plugin_id(), &plugin);

    for unauthorized in [None, Some(""), Some("wrong-token")] {
        let response = fixture.send(request_frame(unauthorized)).await;
        assert_error(&response, 401, "PROXY_UNAUTHORIZED");
    }
    fixture.registry.revoke(&plugin, current.generation());
    assert_error(
        &fixture.send(request_frame(Some(&token))).await,
        401,
        "PROXY_UNAUTHORIZED",
    );
    assert_eq!(
        fixture.backend.seen().len(),
        1,
        "unauthorized input reached backend"
    );

    fixture.shutdown().await;
}

#[tokio::test]
async fn gateway_requires_each_nullable_wire_field_but_accepts_explicit_null() {
    let fixture = GatewayFixture::start(FakeBackend::default(), test_limits()).await;
    let registration = fixture
        .registry
        .register(plugin_id(CONTENT_PLUGIN))
        .unwrap();
    let token = registration.token().with_value(str::to_owned);

    let mut missing_results = Vec::new();
    for field in [
        "token",
        "bodyBase64",
        "cookieJarSessionId",
        "credentialScope",
    ] {
        let mut request = request_frame(Some(&token));
        request.as_object_mut().unwrap().remove(field);
        let response = fixture.send(request).await;
        missing_results.push((
            field,
            response["status"].as_u64().unwrap(),
            response["error"]["code"].as_str().map(str::to_owned),
        ));
    }
    assert_eq!(
        missing_results,
        vec![
            ("token", 400, Some("INVALID_REQUEST".to_string())),
            ("bodyBase64", 400, Some("INVALID_REQUEST".to_string())),
            (
                "cookieJarSessionId",
                400,
                Some("INVALID_REQUEST".to_string())
            ),
            ("credentialScope", 400, Some("INVALID_REQUEST".to_string())),
        ]
    );

    assert_error(
        &fixture.send(request_frame(None)).await,
        401,
        "PROXY_UNAUTHORIZED",
    );
    let accepted = fixture.send(request_frame(Some(&token))).await;
    assert_eq!(accepted["status"], 200);
    assert_eq!(fixture.backend.seen().len(), 1);

    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_response_body_is_rejected_before_base64_or_json_allocation() {
    let success_overhead = br#"{"status":200,"headers":{},"bodyBase64":"","error":null}"#.len();
    let maximum_base64_bytes = ((MAX_PROXY_FRAME_BYTES - success_overhead) / 4) * 4;
    let maximum_body_bytes = maximum_base64_bytes / 4 * 3;
    let impossible_body_bytes = maximum_body_bytes + 1;
    let impossible_base64_bytes = maximum_base64_bytes + 4;
    let backend = LargeBodyBackend::new([
        vec![b'a'; maximum_body_bytes],
        vec![b'b'; impossible_body_bytes],
    ]);
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("proxy/core.sock");
    let registry = Arc::new(ProxyTokenRegistry::new());
    let gateway = ProxyGateway::bind_with_limits(
        &path,
        Arc::clone(&registry),
        Arc::new(backend),
        test_limits(),
    )
    .await
    .unwrap();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let gateway_task = tokio::spawn(gateway.run(shutdown_rx));
    let registration = registry.register(plugin_id(CONTENT_PLUGIN)).unwrap();
    let token = registration.token().with_value(str::to_owned);

    let boundary = send_to(&path, request_frame(Some(&token))).await;
    assert_eq!(boundary["status"], 200);
    assert_eq!(
        boundary["bodyBase64"].as_str().unwrap().len(),
        maximum_base64_bytes
    );

    let tracker = AllocationTracker::start(impossible_base64_bytes);
    let rejected = send_to(&path, request_frame(Some(&token))).await;
    let allocated_impossible_encoding = tracker.finish();
    assert_error(&rejected, 502, "MESSAGE_TOO_LARGE");
    assert!(
        !allocated_impossible_encoding,
        "gateway allocated an impossible full base64/JSON response before rejecting it"
    );

    shutdown_tx.send(true).unwrap();
    gateway_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn gateway_rejects_identity_claims_malformed_modes_headers_and_base64() {
    let fixture = GatewayFixture::start(FakeBackend::default(), test_limits()).await;
    let registration = fixture
        .registry
        .register(plugin_id(CONTENT_PLUGIN))
        .unwrap();
    let token = registration.token().with_value(str::to_owned);

    let mut cases = Vec::new();
    let mut spoofed = request_frame(Some(&token));
    spoofed["pluginId"] = json!("com.example.proxy-fixture.attacker");
    cases.push(spoofed);
    let mut credential_claim = request_frame(Some(&token));
    credential_claim["credentialId"] = json!(Uuid::new_v4().to_string());
    cases.push(credential_claim);
    let mut both_modes = request_frame(Some(&token));
    both_modes["cookieJarSessionId"] = json!(Uuid::new_v4().to_string());
    both_modes["credentialScope"] = json!(SCOPE);
    cases.push(both_modes);
    let mut invalid_body = request_frame(Some(&token));
    invalid_body["bodyBase64"] = json!("not canonical base64===");
    cases.push(invalid_body);
    let mut forbidden_header = request_frame(Some(&token));
    forbidden_header["headers"] = json!({"authorization": "secret"});
    cases.push(forbidden_header);
    let mut duplicate_header = request_frame(Some(&token));
    duplicate_header["headers"] = json!({"x-test": "one", "X-Test": "two"});
    cases.push(duplicate_header);

    for request in cases {
        assert_error(&fixture.send(request).await, 400, "INVALID_REQUEST");
    }
    assert_eq!(fixture.backend.seen().len(), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn unix_framing_is_single_bounded_timed_and_concurrent() {
    let backend = FakeBackend::gated();
    let fixture = GatewayFixture::start(backend, test_limits()).await;
    let registered = fixture
        .registry
        .register(plugin_id(CONTENT_PLUGIN))
        .unwrap();
    let token = registered.token().with_value(str::to_owned);

    let first = serde_json::to_vec(&request_frame(Some(&token))).unwrap();
    let mut two_frames = first.clone();
    two_frames.push(b'\n');
    two_frames.extend_from_slice(&first);
    two_frames.push(b'\n');
    assert_error(&fixture.send_raw(&two_frames).await, 400, "INVALID_REQUEST");
    assert_eq!(fixture.backend.seen().len(), 0);

    assert_error(
        &fixture.send_raw(b"{broken}\n").await,
        400,
        "INVALID_REQUEST",
    );
    let oversized = vec![b'x'; MAX_PROXY_FRAME_BYTES + 2];
    assert_error(
        &fixture.send_raw(&oversized).await,
        413,
        "MESSAGE_TOO_LARGE",
    );

    let idle = UnixStream::connect(&fixture.path).await.unwrap();
    let idle_response = timeout(Duration::from_secs(1), read_response(idle))
        .await
        .unwrap();
    assert_error(&idle_response, 400, "INVALID_REQUEST");

    let one = fixture.spawn_send(request_frame(Some(&token)));
    let two = fixture.spawn_send(request_frame(Some(&token)));
    fixture.backend.wait_for_entries(2).await;
    assert_eq!(
        fixture.backend.max_active(),
        2,
        "connections were serialized"
    );
    let mut rejected = UnixStream::connect(&fixture.path).await.unwrap();
    let mut rejected_frame = serde_json::to_vec(&request_frame(Some(&token))).unwrap();
    rejected_frame.push(b'\n');
    rejected.write_all(&rejected_frame).await.unwrap();
    let mut byte = [0_u8; 1];
    assert_eq!(
        timeout(
            Duration::from_secs(1),
            tokio::io::AsyncReadExt::read(&mut rejected, &mut byte)
        )
        .await
        .unwrap()
        .unwrap(),
        0,
        "connection above the global limit was not closed"
    );
    fixture.backend.release();
    assert_eq!(one.await.unwrap()["status"], 200);
    assert_eq!(two.await.unwrap()["status"], 200);

    fixture.shutdown().await;
}

#[tokio::test]
async fn framing_accepts_exact_limit_and_rejects_one_byte_more() {
    let fixture = GatewayFixture::start(FakeBackend::default(), test_limits()).await;
    let registered = fixture
        .registry
        .register(plugin_id(CONTENT_PLUGIN))
        .unwrap();
    let token = registered.token().with_value(str::to_owned);
    let exact = exact_sized_request(&token);
    assert_eq!(exact.len(), MAX_PROXY_FRAME_BYTES);

    let mut accepted = exact.clone();
    accepted.push(b'\n');
    assert_eq!(fixture.send_raw(&accepted).await["status"], 200);

    let mut oversized = exact;
    oversized.push(b' ');
    oversized.push(b'\n');
    assert_error(
        &fixture.send_raw(&oversized).await,
        413,
        "MESSAGE_TOO_LARGE",
    );
    assert_eq!(fixture.backend.seen().len(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_revokes_tokens_removes_only_its_socket_and_bind_fails_closed() {
    let temporary = tempdir().unwrap();
    let occupied = temporary.path().join("occupied");
    std::fs::write(&occupied, b"do not remove").unwrap();
    let registry = Arc::new(ProxyTokenRegistry::new());
    assert!(
        ProxyGateway::bind(&occupied, registry, Arc::new(FakeBackend::default()))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&occupied).unwrap(), b"do not remove");

    let fixture = GatewayFixture::start(FakeBackend::default(), test_limits()).await;
    let registration = fixture
        .registry
        .register(plugin_id(CONTENT_PLUGIN))
        .unwrap();
    let token = registration.token().with_value(str::to_owned);
    let registry = Arc::clone(&fixture.registry);
    let socket = fixture.path.clone();
    fixture.shutdown().await;
    assert!(!socket.exists());
    assert!(registry.authenticate(&token).is_err());
}

#[tokio::test]
async fn sqlite_adapters_enforce_manifest_grant_origin_and_vault_state() {
    let storage = migrated_storage().await;
    let credential_plugin =
        plugin_record(CREDENTIAL_PLUGIN, PluginType::Credential, "c".repeat(64));
    let content_plugin = plugin_record(CONTENT_PLUGIN, PluginType::Content, "a".repeat(64));
    storage.plugins().upsert(&credential_plugin).await.unwrap();
    storage.plugins().upsert(&content_plugin).await.unwrap();

    let repository = SqliteVaultRepository::new(storage.clone());
    let vault = CredentialVault::new(master_key(), repository.clone());
    let created = vault
        .trusted()
        .create_token(
            CredentialCreateRequest {
                platform_id: "proxyfixture".to_string(),
                scope: scope(),
                source_plugin_id: Some(plugin_id(CREDENTIAL_PLUGIN)),
                target_origins: vec![origin(ORIGIN)],
                account_id_hint: None,
                display_name: Some("Fixture account".to_string()),
                expires_at: None,
            },
            TokenCredentialSecret::bearer(SecretString::new(TOKEN_CANARY.to_string())).unwrap(),
        )
        .await
        .unwrap();
    let grant = CredentialScopeGrantRecord {
        id: Uuid::new_v4(),
        plugin_id: plugin_id(CONTENT_PLUGIN),
        manifest_hash: content_plugin.manifest_hash.clone(),
        credential_id: created.id,
        scope: scope(),
        target_origins: vec![origin(ORIGIN)],
        created_at: Utc::now(),
        revoked_at: None,
    };
    storage.credentials().create_grant(&grant).await.unwrap();

    let loader = SqliteInstalledPluginLoader::new(storage.clone());
    let loaded = loader.load(&plugin_id(CONTENT_PLUGIN)).await.unwrap();
    assert_eq!(loaded.manifest_hash, content_plugin.manifest_hash);
    let selector = SqliteCredentialSelector::new(storage.clone());
    assert_eq!(
        selector
            .select(&scope())
            .await
            .unwrap()
            .unwrap()
            .credential_id,
        created.id
    );
    let vault_port = SqliteCredentialVaultPort::new(vault.clone());
    assert_eq!(
        vault_port
            .open_current(&created.id)
            .await
            .unwrap()
            .unwrap()
            .metadata
            .id,
        created.id
    );
    let grant_port = SqliteCredentialGrantPort::new(storage.clone());
    assert_eq!(
        grant_port
            .active_grant(&plugin_id(CONTENT_PLUGIN), &created.id, &scope())
            .await
            .unwrap()
            .unwrap()
            .target_origins,
        vec![origin(ORIGIN)]
    );

    let transport = ScriptedTransport::with_responses([ok_transport_response()]);
    let resolver = StaticResolver::new([(HOST, vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))])]);
    let backend =
        SqliteCoreProxyBackend::new(storage.clone(), vault.clone(), resolver, transport.clone());
    let runtime = authenticated_runtime(CONTENT_PLUGIN);
    let response = backend
        .execute(
            &runtime,
            core_request(Some(scope()), format!("{ORIGIN}/v1/data")),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let seen = transport.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0]
            .headers
            .get(header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {TOKEN_CANARY}")
    );

    storage
        .credentials()
        .revoke_grant(grant.id, Utc::now())
        .await
        .unwrap();
    let error = backend
        .execute(
            &runtime,
            core_request(Some(scope()), format!("{ORIGIN}/v1/denied")),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, CoreProxyBackendError::PolicyDenied));
    assert_eq!(transport.seen().len(), 1, "revoked grant reached transport");

    let replacement_grant = CredentialScopeGrantRecord {
        id: Uuid::new_v4(),
        created_at: Utc::now(),
        ..grant.clone()
    };
    storage
        .credentials()
        .create_grant(&replacement_grant)
        .await
        .unwrap();
    vault
        .trusted()
        .update_token(
            CredentialUpdateRequest {
                credential_id: created.id,
                expected_revision: created.revision,
                target_origins: vec![origin("https://other.proxy-fixture.invalid")],
                account_id_hint: None,
                display_name: None,
                status: CredentialStatus::Active,
                safe_error_summary: None,
                expires_at: None,
                status_checked_at: Some(Utc::now()),
            },
            TokenCredentialSecret::bearer(SecretString::new(TOKEN_CANARY.to_string())).unwrap(),
        )
        .await
        .unwrap();
    assert!(grant_port
        .active_grant(&plugin_id(CONTENT_PLUGIN), &created.id, &scope())
        .await
        .unwrap()
        .is_none());

    let mut disabled = content_plugin;
    disabled.enabled = false;
    disabled.status = PluginStatus::Disabled;
    storage.plugins().upsert(&disabled).await.unwrap();
    assert!(loader.load(&plugin_id(CONTENT_PLUGIN)).await.is_err());
    assert!(backend
        .execute(
            &runtime,
            core_request(None, format!("{ORIGIN}/v1/disabled"))
        )
        .await
        .is_err());

    let stored = storage
        .credentials()
        .get(&created.id)
        .await
        .unwrap()
        .unwrap();
    let rendered = format!("{stored:?} {error:?} {error}");
    assert!(!rendered.contains(TOKEN_CANARY));
}

#[tokio::test]
async fn sqlite_backend_reloads_current_manifest_instead_of_reusing_stale_policy() {
    let storage = migrated_storage().await;
    let original = plugin_record(CONTENT_PLUGIN, PluginType::Content, "a".repeat(64));
    storage.plugins().upsert(&original).await.unwrap();
    let repository = SqliteVaultRepository::new(storage.clone());
    let vault = CredentialVault::new(master_key(), repository);
    let transport = ScriptedTransport::with_responses([ok_transport_response()]);
    let resolver = StaticResolver::new([
        (HOST, vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))]),
        (
            "replacement.proxy-fixture.invalid",
            vec![IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4))],
        ),
    ]);
    let backend = SqliteCoreProxyBackend::new(storage.clone(), vault, resolver, transport.clone());
    let runtime = authenticated_runtime(CONTENT_PLUGIN);
    backend
        .execute(&runtime, core_request(None, format!("{ORIGIN}/before")))
        .await
        .unwrap();

    let mut replacement = plugin_record(CONTENT_PLUGIN, PluginType::Content, "b".repeat(64));
    replacement.manifest_json["network"]["allowedHosts"] =
        json!(["replacement.proxy-fixture.invalid"]);
    storage.plugins().upsert(&replacement).await.unwrap();
    assert!(backend
        .execute(&runtime, core_request(None, format!("{ORIGIN}/after")))
        .await
        .is_err());
    assert_eq!(
        transport.seen().len(),
        1,
        "stale manifest policy remained cached"
    );
}

#[tokio::test]
async fn sqlite_backend_retains_services_by_complete_manifest_identity() {
    let storage = migrated_storage().await;
    let original = plugin_record(CONTENT_PLUGIN, PluginType::Content, "a".repeat(64));
    storage.plugins().upsert(&original).await.unwrap();
    let loader = SqliteInstalledPluginLoader::new(storage.clone());
    let old_context = loader.load(&plugin_id(CONTENT_PLUGIN)).await.unwrap();

    let mut replacement = plugin_record(CONTENT_PLUGIN, PluginType::Content, "b".repeat(64));
    replacement.manifest_json["network"]["allowedHosts"] =
        json!(["replacement.proxy-fixture.invalid"]);
    storage.plugins().upsert(&replacement).await.unwrap();
    let new_context = loader.load(&plugin_id(CONTENT_PLUGIN)).await.unwrap();

    let repository = SqliteVaultRepository::new(storage.clone());
    let vault = CredentialVault::new(master_key(), repository);
    let resolver = StaticResolver::new([(HOST, vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))])]);
    let backend =
        SqliteCoreProxyBackend::new(storage, vault, resolver, ScriptedTransport::default());
    let original_service = backend.service_for(&old_context).unwrap();
    let current_service = backend.service_for(&new_context).unwrap();

    let delayed_original_service = backend.service_for(&old_context).unwrap();
    let current_service_after_delay = backend.service_for(&new_context).unwrap();

    assert!(Arc::ptr_eq(&original_service, &delayed_original_service));
    assert!(Arc::ptr_eq(&current_service, &current_service_after_delay));
}

#[tokio::test]
async fn sqlite_vault_delete_update_interleaving_cannot_recreate_revision_one() {
    let storage = migrated_storage().await;
    let owner = plugin_record(CREDENTIAL_PLUGIN, PluginType::Credential, "c".repeat(64));
    storage.plugins().upsert(&owner).await.unwrap();
    let repository = SqliteVaultRepository::new(storage.clone());
    let vault = CredentialVault::new(master_key(), repository.clone());
    let created = vault
        .trusted()
        .create_token(
            CredentialCreateRequest {
                platform_id: "proxyfixture".to_string(),
                scope: scope(),
                source_plugin_id: Some(plugin_id(CREDENTIAL_PLUGIN)),
                target_origins: vec![origin(ORIGIN)],
                account_id_hint: None,
                display_name: None,
                expires_at: None,
            },
            TokenCredentialSecret::bearer(SecretString::new(TOKEN_CANARY.to_string())).unwrap(),
        )
        .await
        .unwrap();
    let stale = repository.get(&created.id).await.unwrap().unwrap();

    repository.delete(&created.id).await.unwrap();
    assert!(matches!(
        repository.update(&stale, stale.revision).await,
        Err(CredentialRepositoryError::NotFound)
    ));
    assert!(repository.get(&created.id).await.unwrap().is_none());
}

#[tokio::test]
async fn core_router_exposes_no_proxy_http_route() {
    let storage = migrated_storage().await;
    let state = AppState::new(
        storage,
        semver::Version::new(1, 0, 0),
        Arc::new(UnavailableSupervisorClient),
    );
    let app = build_router(state);
    for path in ["/api/v1/proxy", "/api/v1/internal/proxy"] {
        let response = app
            .clone()
            .oneshot(Request::post(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "route exists: {path}"
        );
    }
}

#[derive(Clone, Default)]
struct FakeBackend {
    seen: Arc<Mutex<Vec<(AuthenticatedRuntime, CoreProxyRequest)>>>,
    entered: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    gate: Option<Arc<Notify>>,
}

impl FakeBackend {
    fn gated() -> Self {
        Self {
            gate: Some(Arc::new(Notify::new())),
            ..Self::default()
        }
    }

    fn seen(&self) -> Vec<(AuthenticatedRuntime, CoreProxyRequest)> {
        self.seen.lock().unwrap().clone()
    }

    async fn wait_for_entries(&self, count: usize) {
        while self.entered.load(Ordering::SeqCst) < count {
            tokio::task::yield_now().await;
        }
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }

    fn release(&self) {
        if let Some(gate) = &self.gate {
            gate.notify_waiters();
        }
    }
}

#[async_trait]
impl CoreProxyBackend for FakeBackend {
    async fn execute(
        &self,
        runtime: &AuthenticatedRuntime,
        request: CoreProxyRequest,
    ) -> Result<CoreProxyResponse, CoreProxyBackendError> {
        self.seen.lock().unwrap().push((runtime.clone(), request));
        self.entered.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(CoreProxyResponse::new(
            StatusCode::OK,
            HeaderMap::from_iter([(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )]),
            b"{}".to_vec(),
        ))
    }
}

#[derive(Clone)]
struct LargeBodyBackend {
    bodies: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl LargeBodyBackend {
    fn new(bodies: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            bodies: Arc::new(Mutex::new(bodies.into_iter().collect())),
        }
    }
}

#[async_trait]
impl CoreProxyBackend for LargeBodyBackend {
    async fn execute(
        &self,
        _runtime: &AuthenticatedRuntime,
        _request: CoreProxyRequest,
    ) -> Result<CoreProxyResponse, CoreProxyBackendError> {
        let body = self
            .bodies
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted response body");
        Ok(CoreProxyResponse::new(
            StatusCode::OK,
            HeaderMap::new(),
            body,
        ))
    }
}

struct GatewayFixture {
    _temporary: TempDir,
    path: std::path::PathBuf,
    registry: Arc<ProxyTokenRegistry>,
    backend: FakeBackend,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<Result<(), audiodown_server::proxy_gateway::ProxyGatewayError>>,
}

impl GatewayFixture {
    async fn start(backend: FakeBackend, limits: ProxyGatewayLimits) -> Self {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("proxy/core.sock");
        let registry = Arc::new(ProxyTokenRegistry::new());
        let gateway = ProxyGateway::bind_with_limits(
            &path,
            Arc::clone(&registry),
            Arc::new(backend.clone()),
            limits,
        )
        .await
        .unwrap();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(gateway.run(shutdown_rx));
        Self {
            _temporary: temporary,
            path,
            registry,
            backend,
            shutdown_tx,
            task,
        }
    }

    async fn send(&self, value: Value) -> Value {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.send_raw(&bytes).await
    }

    fn spawn_send(&self, value: Value) -> JoinHandle<Value> {
        let path = self.path.clone();
        tokio::spawn(async move { send_to(&path, value).await })
    }

    async fn send_raw(&self, bytes: &[u8]) -> Value {
        let mut stream = UnixStream::connect(&self.path).await.unwrap();
        stream.write_all(bytes).await.unwrap();
        read_response(stream).await
    }

    async fn shutdown(self) {
        let registry = Arc::clone(&self.registry);
        self.shutdown_tx.send(true).unwrap();
        self.task.await.unwrap().unwrap();
        assert_eq!(registry.len(), 0);
        assert!(!self.path.exists());
    }
}

fn test_limits() -> ProxyGatewayLimits {
    ProxyGatewayLimits {
        framing_timeout: Duration::from_millis(80),
        write_timeout: Duration::from_secs(1),
        max_connections: 2,
    }
}

async fn send_to(path: &Path, value: Value) -> Value {
    let mut stream = UnixStream::connect(path).await.unwrap();
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).await.unwrap();
    read_response(stream).await
}

async fn read_response(stream: UnixStream) -> Value {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim_end()).unwrap()
}

fn request_frame(token: Option<&str>) -> Value {
    json!({
        "token": token,
        "requestId": "request-1",
        "method": "GET",
        "url": format!("{ORIGIN}/v1/items"),
        "headers": {"accept": "application/json"},
        "bodyBase64": null,
        "cookieJarSessionId": null,
        "credentialScope": null
    })
}

fn exact_sized_request(token: &str) -> Vec<u8> {
    for request_id_bytes in 1..=MAX_REQUEST_ID_BYTES_FOR_TEST {
        let mut request = request_frame(Some(token));
        request["requestId"] = json!("r".repeat(request_id_bytes));
        request["bodyBase64"] = json!("");
        let base = serde_json::to_vec(&request).unwrap();
        let encoded_bytes = MAX_PROXY_FRAME_BYTES - base.len();
        if encoded_bytes % 4 != 0 {
            continue;
        }
        request["bodyBase64"] = json!(
            base64::engine::general_purpose::STANDARD.encode(vec![0_u8; encoded_bytes / 4 * 3])
        );
        let encoded = serde_json::to_vec(&request).unwrap();
        if encoded.len() == MAX_PROXY_FRAME_BYTES {
            return encoded;
        }
    }
    panic!("could not construct an exact-size canonical request frame")
}

const MAX_REQUEST_ID_BYTES_FOR_TEST: usize = 256;

fn assert_error(value: &Value, status: u64, code: &str) {
    assert_eq!(value["status"], status);
    assert_eq!(value["headers"], json!({}));
    assert_eq!(value["bodyBase64"], Value::Null);
    assert_eq!(value["error"]["code"], code);
    assert!(value["error"]["summary"].is_string());
    assert_eq!(value.as_object().unwrap().len(), 4);
}

fn authenticated_runtime(plugin: &str) -> AuthenticatedRuntime {
    let registry = ProxyTokenRegistry::new();
    let registration = registry.register(plugin_id(plugin)).unwrap();
    registry
        .authenticate(&registration.token().with_value(str::to_owned))
        .unwrap()
}

fn core_request(credential_scope: Option<CredentialScope>, url: String) -> CoreProxyRequest {
    CoreProxyRequest {
        request_id: "adapter-request".to_string(),
        method: Method::GET,
        url,
        headers: HeaderMap::new(),
        body: Vec::new(),
        cookie_jar_session_id: None,
        credential_scope,
    }
}

async fn migrated_storage() -> Storage {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    storage.migrate().await.unwrap();
    storage
}

fn plugin_record(id: &str, plugin_type: PluginType, manifest_hash: String) -> PluginRecord {
    let manifest = manifest(id, plugin_type);
    let now = Utc::now();
    PluginRecord {
        plugin_id: manifest.id.clone(),
        plugin_type,
        platform_id: manifest.platform.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.to_string(),
        protocol_version: "1.0".to_string(),
        source_kind: "fixture".to_string(),
        source_ref: "task-12".to_string(),
        commit_sha: None,
        repository_id: None,
        manifest_json: serde_json::to_value(&manifest).unwrap(),
        manifest_hash,
        source_hash: None,
        image_id: Some("sha256:proxy-fixture".to_string()),
        status: PluginStatus::Healthy,
        run_mode: RunMode::OnDemand,
        priority: 100,
        enabled: true,
        last_error: None,
        install_operation_id: None,
        last_used_at: None,
        installed_at: now,
        updated_at: now,
    }
}

fn manifest(id: &str, plugin_type: PluginType) -> PluginManifest {
    let credentials = match plugin_type {
        PluginType::Content => json!({
            "requiredScopes": [{"scope": SCOPE, "targetOrigins": [ORIGIN]}]
        }),
        PluginType::Credential => json!({
            "providedScopes": [{"scope": SCOPE, "targetOrigins": [ORIGIN]}]
        }),
    };
    serde_json::from_value(json!({
        "schemaVersion": "1.0",
        "id": id,
        "name": "Task 12 Proxy Fixture",
        "version": "1.0.0",
        "type": match plugin_type { PluginType::Content => "content", PluginType::Credential => "credential" },
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "proxyfixture", "name": "Proxy Fixture"},
        "capabilities": ["system.health"],
        "network": {"allowedHosts": [HOST]},
        "credentials": credentials
    }))
    .unwrap()
}

fn plugin_id(value: &str) -> PluginId {
    PluginId::parse(value).unwrap()
}

fn scope() -> CredentialScope {
    CredentialScope::parse(SCOPE).unwrap()
}

fn origin(value: &str) -> CredentialTargetOrigin {
    CredentialTargetOrigin::parse(value).unwrap()
}

fn master_key() -> MasterKey {
    MasterKey::from_secret(Secret::new([0x42; 32]))
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
        _target: &PinnedTarget,
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

fn ok_transport_response() -> TransportResponse {
    TransportResponse {
        status: StatusCode::OK,
        headers: HeaderMap::from_iter([(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )]),
        body: b"{}".to_vec(),
    }
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn rendered(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for CapturedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedWriter(Arc::clone(&self.0))
    }
}
