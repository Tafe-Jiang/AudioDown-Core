use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::{
    content::{ContentMethod, SearchResult},
    manifest::PluginType,
    rpc::JsonRpcResponse,
};
use audiodown_plugin_manager::{
    github::GitHubRepositoryRef,
    service::{
        ContentCallEvent, ContentCallLogRecord, ContentInvocationError, ContentInvocationRequest,
        InstallPluginRecord, LifecycleAuthorizationError, LifecycleRiskAuthorizer,
        PluginManagerService, PluginRuntimeControl, PluginStateStore,
    },
    DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use audiodown_supervisor_protocol::{
    PluginInstallOperation, PluginInstallOperationList, PluginRemoveResult, PluginRpcResult,
    PluginRuntimeState,
};
use chrono::{Duration as ChronoDuration, Utc};
use secrecy::SecretString;
use tempfile::TempDir;
use tokio::sync::Notify;
use uuid::Uuid;

#[tokio::test]
async fn validates_plugin_type_enabled_state_and_capability_before_runtime() {
    let fixture = Fixture::new();
    fixture.seed("disabled", PluginType::Content, false, &["content.search"]);
    fixture.seed(
        "credential",
        PluginType::Credential,
        true,
        &["system.health"],
    );
    fixture.seed("missing", PluginType::Content, true, &["content.discover"]);

    for (suffix, expected) in [
        ("disabled", ContentInvocationError::PluginDisabled),
        ("credential", ContentInvocationError::NotContentPlugin),
        ("missing", ContentInvocationError::CapabilityMissing),
    ] {
        let result = fixture
            .service
            .invoke_content(request(suffix, ContentMethod::Search))
            .await;
        assert_eq!(result.unwrap_err(), expected);
    }
    assert!(fixture.runtime.events().is_empty());
}

#[tokio::test]
async fn starts_on_demand_invokes_touches_and_records_call_logs() {
    let fixture = Fixture::new();
    fixture.seed_with_status(
        "ondemand",
        PluginType::Content,
        true,
        &["content.search"],
        PluginStatus::Stopped,
    );

    let result = fixture
        .service
        .invoke_content(request("ondemand", ContentMethod::Search))
        .await
        .unwrap();
    let search: SearchResult = serde_json::from_value(result.response.result.unwrap()).unwrap();
    assert!(search.items.is_empty());
    assert_eq!(
        fixture.runtime.events(),
        [
            "start:com.audiodown.virtual.ondemand",
            "inspect:com.audiodown.virtual.ondemand",
            "invoke:com.audiodown.virtual.ondemand:content.search",
        ]
    );
    assert!(fixture
        .store
        .get("com.audiodown.virtual.ondemand")
        .last_used_at
        .is_some());
    assert_eq!(
        fixture
            .store
            .call_logs()
            .iter()
            .map(|log| log.event)
            .collect::<Vec<_>>(),
        [ContentCallEvent::Started, ContentCallEvent::Succeeded]
    );
}

#[tokio::test]
async fn healthy_plugin_calls_are_not_serialized() {
    let fixture = Arc::new(Fixture::new());
    fixture.seed("parallel", PluginType::Content, true, &["content.search"]);
    fixture.runtime.block_invocation();

    let first = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            fixture
                .service
                .invoke_content(request("parallel", ContentMethod::Search))
                .await
        })
    };
    fixture.runtime.wait_for_invocations(1).await;
    let second = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            fixture
                .service
                .invoke_content(request("parallel", ContentMethod::Search))
                .await
        })
    };
    fixture.runtime.wait_for_invocations(2).await;

    assert_eq!(fixture.runtime.max_active_invocations(), 2);
    fixture.runtime.release_invocations();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert!(!fixture
        .runtime
        .events()
        .iter()
        .any(|event| event.starts_with("start:")));
}

#[tokio::test]
async fn concurrent_cold_calls_share_one_start_and_then_run_in_parallel() {
    let fixture = Arc::new(Fixture::new());
    fixture.seed_with_status(
        "cold-parallel",
        PluginType::Content,
        true,
        &["content.search"],
        PluginStatus::Stopped,
    );
    fixture.runtime.block_start();
    fixture.runtime.block_invocation();

    let first = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            fixture
                .service
                .invoke_content(request("cold-parallel", ContentMethod::Search))
                .await
        })
    };
    fixture.runtime.wait_for_starts(1).await;
    let second = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            fixture
                .service
                .invoke_content(request("cold-parallel", ContentMethod::Search))
                .await
        })
    };

    tokio::task::yield_now().await;
    assert_eq!(fixture.runtime.start_count(), 1);
    fixture.runtime.release_starts();
    let both_calls_entered = tokio::time::timeout(
        Duration::from_secs(1),
        fixture.runtime.wait_for_invocations(2),
    )
    .await
    .is_ok();
    fixture.runtime.release_invocations();
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert!(
        both_calls_entered,
        "both cold calls should reach the runtime after one shared start"
    );
    assert_eq!(fixture.runtime.max_active_invocations(), 2);
    first.unwrap();
    second.unwrap();
    assert_eq!(fixture.runtime.start_count(), 1);

    let report = fixture
        .service
        .reconcile_due_plugins(
            Utc::now() + ChronoDuration::hours(1),
            Duration::from_secs(900),
        )
        .await
        .unwrap();
    assert_eq!(report.stopped, 1);
}

#[tokio::test]
async fn active_call_prevents_idle_stop_until_the_lease_is_released() {
    let fixture = Arc::new(Fixture::new());
    fixture.seed("leased", PluginType::Content, true, &["content.search"]);
    fixture.runtime.block_invocation();
    let invocation = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            fixture
                .service
                .invoke_content(request("leased", ContentMethod::Search))
                .await
        })
    };
    fixture.runtime.wait_for_invocations(1).await;

    let report = fixture
        .service
        .reconcile_due_plugins(
            Utc::now() + ChronoDuration::hours(1),
            Duration::from_secs(900),
        )
        .await
        .unwrap();
    assert_eq!(report.stopped, 0);
    assert!(!fixture
        .runtime
        .events()
        .iter()
        .any(|event| event.starts_with("stop:")));

    fixture.runtime.release_invocations();
    invocation.await.unwrap().unwrap();
    let report = fixture
        .service
        .reconcile_due_plugins(
            Utc::now() + ChronoDuration::hours(1),
            Duration::from_secs(900),
        )
        .await
        .unwrap();
    assert_eq!(report.stopped, 1);
}

#[tokio::test]
async fn runtime_failure_records_a_safe_log_and_releases_the_lease() {
    let fixture = Fixture::new();
    fixture.seed("failure", PluginType::Content, true, &["content.search"]);
    fixture.runtime.fail_invocation();

    let result = fixture
        .service
        .invoke_content(request("failure", ContentMethod::Search))
        .await;
    assert_eq!(
        result.unwrap_err(),
        ContentInvocationError::RuntimeUnavailable
    );
    let logs = fixture.store.call_logs();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[1].event, ContentCallEvent::Failed);
    assert_eq!(logs[1].error_code.as_deref(), Some("PLUGIN_UNAVAILABLE"));
    assert!(!format!("{logs:?}").contains("secret runtime detail"));

    let report = fixture
        .service
        .reconcile_due_plugins(
            Utc::now() + ChronoDuration::hours(1),
            Duration::from_secs(900),
        )
        .await
        .unwrap();
    assert_eq!(report.stopped, 1);
}

struct Fixture {
    _temp: TempDir,
    store: Arc<FakeStore>,
    runtime: Arc<FakeRuntime>,
    service: Arc<PluginManagerService>,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let store = Arc::new(FakeStore::default());
        let runtime = Arc::new(FakeRuntime::default());
        let service = Arc::new(
            PluginManagerService::new(
                store.clone(),
                Arc::new(NoopSource),
                temp.path().join("plugins"),
                "1.0.0-alpha.1".parse().unwrap(),
                "1.0.0".parse().unwrap(),
            )
            .with_installation_ports(runtime.clone(), Arc::new(NoopAuthorizer)),
        );
        Self {
            _temp: temp,
            store,
            runtime,
            service,
        }
    }

    fn seed(&self, suffix: &str, plugin_type: PluginType, enabled: bool, capabilities: &[&str]) {
        self.seed_with_status(
            suffix,
            plugin_type,
            enabled,
            capabilities,
            PluginStatus::Healthy,
        );
    }

    fn seed_with_status(
        &self,
        suffix: &str,
        plugin_type: PluginType,
        enabled: bool,
        capabilities: &[&str],
        status: PluginStatus,
    ) {
        let plugin_id = PluginId::parse(format!("com.audiodown.virtual.{suffix}")).unwrap();
        self.store.seed(record(
            plugin_id,
            plugin_type,
            enabled,
            capabilities,
            status,
        ));
    }
}

fn request(suffix: &str, method: ContentMethod) -> ContentInvocationRequest {
    ContentInvocationRequest {
        request_id: Uuid::new_v4().to_string(),
        plugin_id: PluginId::parse(format!("com.audiodown.virtual.{suffix}")).unwrap(),
        method,
        params: serde_json::json!({"query": "virtual", "limit": 20}),
    }
}

fn record(
    plugin_id: PluginId,
    plugin_type: PluginType,
    enabled: bool,
    capabilities: &[&str],
    status: PluginStatus,
) -> InstallPluginRecord {
    let now = Utc::now() - ChronoDuration::hours(1);
    InstallPluginRecord {
        operation_id: Uuid::nil(),
        plugin_id: plugin_id.clone(),
        plugin_type,
        platform_id: "virtual".to_string(),
        name: "Virtual Content".to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_ref: "virtual".to_string(),
        commit_sha: "0".repeat(40),
        repository_id: "virtual.plugins".to_string(),
        manifest_json: serde_json::json!({
            "schemaVersion": "1.0",
            "id": plugin_id,
            "name": "Virtual Content",
            "version": "1.0.0",
            "type": match plugin_type {
                PluginType::Content => "content",
                PluginType::Credential => "credential",
            },
            "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
            "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
            "platform": {"id": "virtual", "name": "Virtual"},
            "capabilities": capabilities,
            "network": {"allowedHosts": []}
        }),
        manifest_hash: "a".repeat(64),
        source_hash: "b".repeat(64),
        image_id: Some(format!("sha256:{}", "c".repeat(64))),
        status,
        run_mode: RunMode::OnDemand,
        priority: 100,
        enabled,
        last_error: None,
        install_operation_id: None,
        last_used_at: Some(now),
        installed_at: now,
        updated_at: now,
    }
}

#[derive(Default)]
struct FakeStore {
    records: Mutex<HashMap<PluginId, InstallPluginRecord>>,
    call_logs: Mutex<Vec<ContentCallLogRecord>>,
}

impl FakeStore {
    fn seed(&self, record: InstallPluginRecord) {
        self.records
            .lock()
            .unwrap()
            .insert(record.plugin_id.clone(), record);
    }

    fn get(&self, plugin_id: &str) -> InstallPluginRecord {
        self.records
            .lock()
            .unwrap()
            .get(&PluginId::parse(plugin_id).unwrap())
            .unwrap()
            .clone()
    }

    fn call_logs(&self) -> Vec<ContentCallLogRecord> {
        self.call_logs.lock().unwrap().clone()
    }
}

#[async_trait]
impl PluginStateStore for FakeStore {
    async fn is_installed(&self, plugin_id: &PluginId) -> Result<bool, PluginManagerError> {
        Ok(self.records.lock().unwrap().contains_key(plugin_id))
    }

    async fn list_install_records(&self) -> Result<Vec<InstallPluginRecord>, PluginManagerError> {
        Ok(self.records.lock().unwrap().values().cloned().collect())
    }

    async fn get_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<InstallPluginRecord>, PluginManagerError> {
        Ok(self.records.lock().unwrap().get(plugin_id).cloned())
    }

    async fn save_plugin(&self, record: &InstallPluginRecord) -> Result<(), PluginManagerError> {
        self.seed(record.clone());
        Ok(())
    }

    async fn touch(
        &self,
        plugin_id: &PluginId,
        last_used_at: chrono::DateTime<Utc>,
    ) -> Result<(), PluginManagerError> {
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(plugin_id)
            .ok_or(PluginManagerError::PluginStateUnavailable)?;
        record.last_used_at = Some(last_used_at);
        record.updated_at = last_used_at;
        Ok(())
    }

    async fn persist_content_call_log(
        &self,
        record: &ContentCallLogRecord,
    ) -> Result<(), PluginManagerError> {
        self.call_logs.lock().unwrap().push(record.clone());
        Ok(())
    }
}

#[derive(Default)]
struct FakeRuntime {
    events: Mutex<Vec<String>>,
    fail_invoke: AtomicBool,
    block_start: AtomicBool,
    block_invoke: AtomicBool,
    active_starts: AtomicUsize,
    active_invocations: AtomicUsize,
    max_active_invocations: AtomicUsize,
    start_entered: Notify,
    start_release: Notify,
    invocation_entered: Notify,
    invocation_release: Notify,
}

impl FakeRuntime {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn fail_invocation(&self) {
        self.fail_invoke.store(true, Ordering::SeqCst);
    }

    fn block_start(&self) {
        self.block_start.store(true, Ordering::SeqCst);
    }

    fn block_invocation(&self) {
        self.block_invoke.store(true, Ordering::SeqCst);
    }

    async fn wait_for_starts(&self, count: usize) {
        while self.active_starts.load(Ordering::SeqCst) < count {
            self.start_entered.notified().await;
        }
    }

    fn start_count(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.starts_with("start:"))
            .count()
    }

    fn release_starts(&self) {
        self.block_start.store(false, Ordering::SeqCst);
        self.start_release.notify_waiters();
    }

    async fn wait_for_invocations(&self, count: usize) {
        while self.active_invocations.load(Ordering::SeqCst) < count {
            self.invocation_entered.notified().await;
        }
    }

    fn max_active_invocations(&self) -> usize {
        self.max_active_invocations.load(Ordering::SeqCst)
    }

    fn release_invocations(&self) {
        self.block_invoke.store(false, Ordering::SeqCst);
        self.invocation_release.notify_waiters();
    }

    fn runtime_state(plugin_id: &PluginId, status: PluginStatus) -> PluginRuntimeState {
        PluginRuntimeState {
            plugin_id: plugin_id.clone(),
            status,
            container_id: Some("container-id".to_string()),
            logs: Vec::new(),
        }
    }
}

#[async_trait]
impl PluginRuntimeControl for FakeRuntime {
    async fn start(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("start:{plugin_id}"));
        self.active_starts.fetch_add(1, Ordering::SeqCst);
        self.start_entered.notify_waiters();
        if self.block_start.load(Ordering::SeqCst) {
            self.start_release.notified().await;
        }
        self.active_starts.fetch_sub(1, Ordering::SeqCst);
        Ok(Self::runtime_state(plugin_id, PluginStatus::Healthy))
    }

    async fn stop(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("stop:{plugin_id}"));
        Ok(Self::runtime_state(plugin_id, PluginStatus::Stopped))
    }

    async fn inspect(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("inspect:{plugin_id}"));
        Ok(Self::runtime_state(plugin_id, PluginStatus::Healthy))
    }

    async fn invoke(
        &self,
        plugin_id: &PluginId,
        method: ContentMethod,
        _params: serde_json::Value,
    ) -> Result<PluginRpcResult, PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("invoke:{plugin_id}:{}", method.capability()));
        let active = self.active_invocations.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active_invocations
            .fetch_max(active, Ordering::SeqCst);
        self.invocation_entered.notify_waiters();
        if self.block_invoke.load(Ordering::SeqCst) {
            self.invocation_release.notified().await;
        }
        self.active_invocations.fetch_sub(1, Ordering::SeqCst);
        if self.fail_invoke.load(Ordering::SeqCst) {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        PluginRpcResult::new(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Uuid::new_v4().to_string(),
            result: Some(serde_json::json!({"items": [], "nextCursor": null})),
            error: None,
        })
        .map_err(|_| PluginManagerError::RuntimeUnavailable)
    }

    async fn remove(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRemoveResult, PluginManagerError> {
        unimplemented!()
    }

    async fn begin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        unimplemented!()
    }

    async fn install_status(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        unimplemented!()
    }

    async fn finalize_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        unimplemented!()
    }

    async fn abort_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        unimplemented!()
    }

    async fn list_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, PluginManagerError> {
        unimplemented!()
    }

    async fn acknowledge_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        unimplemented!()
    }
}

struct NoopSource;

#[async_trait]
impl RepositorySource for NoopSource {
    async fn resolve_and_download(
        &self,
        _source: &GitHubRepositoryRef,
        _destination: &Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError> {
        unimplemented!()
    }
}

struct NoopAuthorizer;

#[async_trait]
impl LifecycleRiskAuthorizer for NoopAuthorizer {
    async fn authorize(
        &self,
        _token: Option<&SecretString>,
    ) -> Result<(), LifecycleAuthorizationError> {
        Ok(())
    }
}
