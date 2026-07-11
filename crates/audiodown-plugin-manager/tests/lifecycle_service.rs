use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_plugin_manager::{
    github::GitHubRepositoryRef,
    service::{
        InstallPluginRecord, LifecycleAuthorizationError, LifecycleRiskAuthorizer,
        PluginManagerService, PluginRuntimeControl, PluginRuntimeLogRecord, PluginStateStore,
    },
    DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use audiodown_supervisor_protocol::{
    PluginInstallOperation, PluginInstallOperationList, PluginRemoveResult, PluginRuntimeState,
};
use chrono::{Duration as ChronoDuration, Utc};
use secrecy::SecretString;
use tempfile::TempDir;
use tokio::sync::Notify;
use uuid::Uuid;

#[tokio::test(start_paused = true)]
async fn reconciles_enabled_always_and_idle_on_demand_plugins() {
    let fixture = Fixture::new();
    let now = Utc::now();
    fixture.seed(
        "always-start",
        true,
        RunMode::Always,
        PluginStatus::Installed,
        None,
    );
    fixture.seed(
        "always-healthy",
        true,
        RunMode::Always,
        PluginStatus::Healthy,
        None,
    );
    fixture.seed(
        "always-disabled",
        false,
        RunMode::Always,
        PluginStatus::Disabled,
        None,
    );
    fixture.seed(
        "on-demand-recent",
        true,
        RunMode::OnDemand,
        PluginStatus::Healthy,
        Some(now - ChronoDuration::seconds(899)),
    );
    fixture.seed(
        "on-demand-idle",
        true,
        RunMode::OnDemand,
        PluginStatus::Healthy,
        Some(now - ChronoDuration::seconds(901)),
    );
    fixture.seed(
        "on-demand-stopped",
        true,
        RunMode::OnDemand,
        PluginStatus::Stopped,
        Some(now - ChronoDuration::hours(1)),
    );
    fixture.seed(
        "installing",
        true,
        RunMode::Always,
        PluginStatus::Installing,
        None,
    );

    let report = fixture
        .service
        .reconcile_due_plugins(now, Duration::from_secs(900))
        .await
        .unwrap();

    assert_eq!(
        fixture.runtime.events(),
        vec![
            "start:com.audiodown.virtual.always-start",
            "inspect:com.audiodown.virtual.always-start",
            "stop:com.audiodown.virtual.on-demand-idle",
        ]
    );
    assert_eq!(report.started, 1);
    assert_eq!(report.stopped, 1);
    assert_eq!(
        fixture.store.status("com.audiodown.virtual.always-start"),
        PluginStatus::Healthy
    );
    assert_eq!(
        fixture.store.status("com.audiodown.virtual.on-demand-idle"),
        PluginStatus::Stopped
    );
}

#[tokio::test(start_paused = true)]
async fn stops_after_three_consecutive_automatic_start_failures() {
    let fixture = Fixture::new();
    let now = Utc::now();
    fixture.seed(
        "retry",
        true,
        RunMode::Always,
        PluginStatus::Installed,
        None,
    );
    fixture.runtime.fail_start();

    for _ in 0..4 {
        fixture
            .service
            .reconcile_due_plugins(now, Duration::from_secs(900))
            .await
            .unwrap();
    }

    assert_eq!(
        fixture
            .runtime
            .events()
            .iter()
            .filter(|event| event.starts_with("start:"))
            .count(),
        3
    );
    let record = fixture.store.get("com.audiodown.virtual.retry");
    assert_eq!(record.status, PluginStatus::Unhealthy);
    assert_eq!(record.run_mode, RunMode::Always);
    assert!(record.enabled);
    assert_eq!(
        record.last_error.as_deref(),
        Some("plugin runtime action failed")
    );
}

#[tokio::test(start_paused = true)]
async fn skips_reconciliation_while_a_user_operation_holds_the_plugin_lock() {
    let fixture = Arc::new(Fixture::new());
    let now = Utc::now();
    fixture.seed("busy", true, RunMode::Always, PluginStatus::Installed, None);
    fixture.runtime.block_start();
    let plugin_id = PluginId::parse("com.audiodown.virtual.busy").unwrap();
    let user_start = {
        let fixture = fixture.clone();
        let plugin_id = plugin_id.clone();
        tokio::spawn(async move { fixture.service.start(&plugin_id).await })
    };
    fixture.runtime.wait_until_start().await;

    let report = fixture
        .service
        .reconcile_due_plugins(now, Duration::from_secs(900))
        .await
        .unwrap();
    assert_eq!(report.skipped_busy, 1);
    assert_eq!(
        fixture
            .runtime
            .events()
            .iter()
            .filter(|event| event.starts_with("start:"))
            .count(),
        1
    );

    fixture.runtime.release_start();
    user_start.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn supervisor_failure_preserves_settings_and_records_a_redacted_error() {
    let fixture = Fixture::new();
    let now = Utc::now();
    fixture.seed(
        "idle-failure",
        true,
        RunMode::OnDemand,
        PluginStatus::Healthy,
        Some(now - ChronoDuration::hours(1)),
    );
    fixture.runtime.fail_stop();

    let report = fixture
        .service
        .reconcile_due_plugins(now, Duration::from_secs(900))
        .await
        .unwrap();
    assert_eq!(report.failed, 1);
    let record = fixture.store.get("com.audiodown.virtual.idle-failure");
    assert_eq!(record.status, PluginStatus::Healthy);
    assert_eq!(record.run_mode, RunMode::OnDemand);
    assert!(record.enabled);
    assert_eq!(
        record.last_error.as_deref(),
        Some("plugin runtime action failed")
    );
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

    fn seed(
        &self,
        suffix: &str,
        enabled: bool,
        run_mode: RunMode,
        status: PluginStatus,
        last_used_at: Option<chrono::DateTime<Utc>>,
    ) {
        self.store.seed(record(
            PluginId::parse(format!("com.audiodown.virtual.{suffix}")).unwrap(),
            enabled,
            run_mode,
            status,
            last_used_at,
        ));
    }
}

fn record(
    plugin_id: PluginId,
    enabled: bool,
    run_mode: RunMode,
    status: PluginStatus,
    last_used_at: Option<chrono::DateTime<Utc>>,
) -> InstallPluginRecord {
    let now = Utc::now();
    InstallPluginRecord {
        operation_id: Uuid::nil(),
        plugin_id,
        plugin_type: PluginType::Content,
        platform_id: "virtual".to_string(),
        name: "Virtual Content".to_string(),
        version: "1.0.0".to_string(),
        protocol_version: "1.0".to_string(),
        source_ref: "https://github.com/example-owner/example-repository".to_string(),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        repository_id: "example.plugins".to_string(),
        manifest_json: serde_json::json!({}),
        manifest_hash: "a".repeat(64),
        source_hash: "b".repeat(64),
        image_id: Some(format!("sha256:{}", "c".repeat(64))),
        status,
        run_mode,
        priority: 100,
        enabled,
        last_error: None,
        install_operation_id: None,
        last_used_at,
        installed_at: now,
        updated_at: now,
    }
}

#[derive(Default)]
struct FakeStore {
    records: Mutex<HashMap<PluginId, InstallPluginRecord>>,
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

    fn status(&self, plugin_id: &str) -> PluginStatus {
        self.get(plugin_id).status
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

    async fn persist_runtime_log(
        &self,
        _record: &PluginRuntimeLogRecord,
    ) -> Result<(), PluginManagerError> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeRuntime {
    events: Mutex<Vec<String>>,
    fail_start: AtomicBool,
    fail_stop: AtomicBool,
    block_start: AtomicBool,
    start_entered: Notify,
    start_release: Notify,
}

impl FakeRuntime {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn fail_start(&self) {
        self.fail_start.store(true, Ordering::SeqCst);
    }

    fn fail_stop(&self) {
        self.fail_stop.store(true, Ordering::SeqCst);
    }

    fn block_start(&self) {
        self.block_start.store(true, Ordering::SeqCst);
    }

    async fn wait_until_start(&self) {
        self.start_entered.notified().await;
    }

    fn release_start(&self) {
        self.start_release.notify_waiters();
    }
}

#[async_trait]
impl PluginRuntimeControl for FakeRuntime {
    async fn start(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("start:{plugin_id}"));
        if self.block_start.load(Ordering::SeqCst) {
            self.start_entered.notify_waiters();
            self.start_release.notified().await;
        }
        if self.fail_start.load(Ordering::SeqCst) {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        Ok(runtime(plugin_id, PluginStatus::Healthy))
    }

    async fn stop(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("stop:{plugin_id}"));
        if self.fail_stop.load(Ordering::SeqCst) {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        Ok(runtime(plugin_id, PluginStatus::Stopped))
    }

    async fn inspect(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("inspect:{plugin_id}"));
        Ok(runtime(plugin_id, PluginStatus::Healthy))
    }

    async fn remove(&self, plugin_id: &PluginId) -> Result<PluginRemoveResult, PluginManagerError> {
        Ok(PluginRemoveResult {
            plugin_id: plugin_id.clone(),
            removed_container: true,
            removed_image: true,
            removed_install_directory: true,
        })
    }

    async fn begin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn install_status(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn finalize_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn abort_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn list_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, PluginManagerError> {
        Ok(PluginInstallOperationList::new(Vec::new()))
    }

    async fn acknowledge_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }
}

fn runtime(plugin_id: &PluginId, status: PluginStatus) -> PluginRuntimeState {
    PluginRuntimeState {
        plugin_id: plugin_id.clone(),
        status,
        container_id: Some("container-virtual".to_string()),
        logs: Vec::new(),
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
        Err(PluginManagerError::RepositoryRequest)
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
