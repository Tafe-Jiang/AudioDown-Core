use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus, RunMode};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_plugin_manager::{
    github::GitHubRepositoryRef,
    service::{
        InstallPluginRecord, LifecycleAuthorizationError, LifecycleRiskAuthorizer,
        PluginManagementError, PluginManagerService, PluginRuntimeControl, PluginRuntimeLogRecord,
        PluginStateStore, UpdatePluginSettingsCommand,
    },
    DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use audiodown_supervisor_protocol::{
    PluginInstallOperation, PluginInstallOperationList, PluginRemoveResult, PluginRuntimeLog,
    PluginRuntimeState,
};
use chrono::Utc;
use secrecy::SecretString;
use tempfile::TempDir;
use tokio::sync::Notify;
use uuid::Uuid;

#[tokio::test]
async fn owns_manual_lifecycle_and_runtime_inspection_transitions() {
    let fixture = Fixture::new();

    let started = fixture.service.start(&fixture.plugin_id).await.unwrap();
    assert_eq!(started.status, PluginStatus::Healthy);
    assert!(started.last_used_at.is_some());
    assert_eq!(fixture.runtime.events(), vec!["start", "inspect"]);
    assert_eq!(fixture.store.runtime_logs().len(), 2);
    assert!(fixture
        .store
        .runtime_logs()
        .iter()
        .all(|log| !log.message.contains("secret-token")));

    let stopped = fixture.service.stop(&fixture.plugin_id).await.unwrap();
    assert_eq!(stopped.status, PluginStatus::Stopped);
    assert_eq!(fixture.runtime.events(), vec!["start", "inspect", "stop"]);

    let inspected = fixture
        .service
        .inspect_runtime(&fixture.plugin_id)
        .await
        .unwrap();
    assert_eq!(inspected.status, PluginStatus::Healthy);
    assert_eq!(
        fixture.runtime.events(),
        vec!["start", "inspect", "stop", "inspect"]
    );
}

#[tokio::test]
async fn validates_and_applies_settings_in_runtime_safe_order() {
    let invalid = Fixture::new();
    let error = invalid
        .service
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id: invalid.plugin_id.clone(),
            enabled: true,
            run_mode: RunMode::OnDemand,
            priority: 1001,
        })
        .await
        .unwrap_err();
    assert_eq!(error, PluginManagementError::InvalidPriority);
    assert!(invalid.runtime.events().is_empty());

    let disabled = Fixture::new();
    disabled.store.mutate(&disabled.plugin_id, |record| {
        record.status = PluginStatus::Healthy;
    });
    let record = disabled
        .service
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id: disabled.plugin_id.clone(),
            enabled: false,
            run_mode: RunMode::OnDemand,
            priority: 25,
        })
        .await
        .unwrap();
    assert!(!record.enabled);
    assert_eq!(record.status, PluginStatus::Disabled);
    assert_eq!(disabled.runtime.events(), vec!["stop"]);

    let enabled = Fixture::new();
    enabled.store.mutate(&enabled.plugin_id, |record| {
        record.enabled = false;
        record.status = PluginStatus::Disabled;
    });
    let record = enabled
        .service
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id: enabled.plugin_id.clone(),
            enabled: true,
            run_mode: RunMode::OnDemand,
            priority: 75,
        })
        .await
        .unwrap();
    assert!(record.enabled);
    assert_eq!(record.status, PluginStatus::Stopped);
    assert!(enabled.runtime.events().is_empty());

    let always = Fixture::new();
    let record = always
        .service
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id: always.plugin_id.clone(),
            enabled: true,
            run_mode: RunMode::Always,
            priority: 10,
        })
        .await
        .unwrap();
    assert_eq!(record.run_mode, RunMode::Always);
    assert_eq!(record.status, PluginStatus::Healthy);
    assert_eq!(always.runtime.events(), vec!["start", "inspect"]);

    let on_demand = Fixture::new();
    on_demand.store.mutate(&on_demand.plugin_id, |record| {
        record.run_mode = RunMode::Always;
        record.status = PluginStatus::Healthy;
    });
    let record = on_demand
        .service
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id: on_demand.plugin_id.clone(),
            enabled: true,
            run_mode: RunMode::OnDemand,
            priority: 100,
        })
        .await
        .unwrap();
    assert_eq!(record.run_mode, RunMode::OnDemand);
    assert_eq!(record.status, PluginStatus::Healthy);
    assert!(on_demand.runtime.events().is_empty());
}

#[tokio::test]
async fn runtime_failures_preserve_settings_and_record_redacted_errors() {
    let fixture = Fixture::new();
    fixture.runtime.fail_start();

    let error = fixture
        .service
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id: fixture.plugin_id.clone(),
            enabled: true,
            run_mode: RunMode::Always,
            priority: 7,
        })
        .await
        .unwrap_err();
    assert_eq!(error, PluginManagementError::RuntimeUnavailable);

    let record = fixture.store.record(&fixture.plugin_id).unwrap();
    assert_eq!(record.run_mode, RunMode::OnDemand);
    assert_eq!(record.priority, 100);
    let last_error = record.last_error.unwrap();
    assert!(!last_error.contains("secret-token"));
    assert_eq!(last_error, "plugin runtime action failed");
}

#[tokio::test]
async fn uninstall_removes_runtime_assets_before_sqlite_and_preserves_failures() {
    let fixture = Fixture::new();
    fixture.service.uninstall(&fixture.plugin_id).await.unwrap();
    assert_eq!(fixture.runtime.events(), vec!["stop", "remove"]);
    assert!(fixture.store.record(&fixture.plugin_id).is_none());
    assert_eq!(fixture.store.events(), vec!["save:stopped", "delete"]);

    let failed = Fixture::new();
    failed.runtime.fail_remove();
    let error = failed
        .service
        .uninstall(&failed.plugin_id)
        .await
        .unwrap_err();
    assert_eq!(error, PluginManagementError::RuntimeUnavailable);
    let record = failed.store.record(&failed.plugin_id).unwrap();
    assert_eq!(
        record.last_error.as_deref(),
        Some("plugin runtime action failed")
    );
}

#[tokio::test]
async fn every_management_operation_shares_the_install_operation_lock() {
    let fixture = Arc::new(Fixture::new());
    fixture.runtime.block_start();
    let running = {
        let fixture = fixture.clone();
        tokio::spawn(async move { fixture.service.start(&fixture.plugin_id).await })
    };
    fixture.runtime.wait_until_start().await;

    let error = fixture
        .service
        .update_settings(UpdatePluginSettingsCommand {
            plugin_id: fixture.plugin_id.clone(),
            enabled: true,
            run_mode: RunMode::OnDemand,
            priority: 1,
        })
        .await
        .unwrap_err();
    assert_eq!(error, PluginManagementError::PluginOperationInProgress);

    fixture.runtime.release_start();
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn reports_unknown_plugins_without_runtime_calls() {
    let fixture = Fixture::new();
    let missing = PluginId::parse("com.audiodown.virtual.missing").unwrap();
    assert_eq!(
        fixture.service.start(&missing).await.unwrap_err(),
        PluginManagementError::PluginNotFound
    );
    assert_eq!(
        fixture.service.uninstall(&missing).await.unwrap_err(),
        PluginManagementError::PluginNotFound
    );
    assert!(fixture.runtime.events().is_empty());
}

struct Fixture {
    _temp: TempDir,
    plugin_id: PluginId,
    store: Arc<FakeStore>,
    runtime: Arc<FakeRuntime>,
    service: Arc<PluginManagerService>,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let plugin_id = PluginId::parse("com.audiodown.virtual.content").unwrap();
        let store = Arc::new(FakeStore::default());
        store.seed(installed_record(plugin_id.clone()));
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
            plugin_id,
            store,
            runtime,
            service,
        }
    }
}

fn installed_record(plugin_id: PluginId) -> InstallPluginRecord {
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
        status: PluginStatus::Installed,
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

#[derive(Default)]
struct FakeStore {
    records: Mutex<HashMap<PluginId, InstallPluginRecord>>,
    events: Mutex<Vec<String>>,
    runtime_logs: Mutex<Vec<PluginRuntimeLogRecord>>,
}

impl FakeStore {
    fn seed(&self, record: InstallPluginRecord) {
        self.records
            .lock()
            .unwrap()
            .insert(record.plugin_id.clone(), record);
    }

    fn mutate(&self, plugin_id: &PluginId, mutate: impl FnOnce(&mut InstallPluginRecord)) {
        mutate(self.records.lock().unwrap().get_mut(plugin_id).unwrap());
    }

    fn record(&self, plugin_id: &PluginId) -> Option<InstallPluginRecord> {
        self.records.lock().unwrap().get(plugin_id).cloned()
    }

    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn runtime_logs(&self) -> Vec<PluginRuntimeLogRecord> {
        self.runtime_logs.lock().unwrap().clone()
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
        Ok(self.record(plugin_id))
    }

    async fn save_plugin(&self, record: &InstallPluginRecord) -> Result<(), PluginManagerError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("save:{:?}", record.status).to_ascii_lowercase());
        self.seed(record.clone());
        Ok(())
    }

    async fn delete_plugin(&self, plugin_id: &PluginId) -> Result<(), PluginManagerError> {
        self.events.lock().unwrap().push("delete".to_string());
        self.records.lock().unwrap().remove(plugin_id);
        Ok(())
    }

    async fn persist_runtime_log(
        &self,
        record: &PluginRuntimeLogRecord,
    ) -> Result<(), PluginManagerError> {
        self.runtime_logs.lock().unwrap().push(record.clone());
        Ok(())
    }
}

#[derive(Default)]
struct FakeRuntime {
    events: Mutex<Vec<&'static str>>,
    fail_start: AtomicBool,
    fail_remove: AtomicBool,
    block_start: AtomicBool,
    start_entered: Notify,
    start_release: Notify,
}

impl FakeRuntime {
    fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }

    fn fail_start(&self) {
        self.fail_start.store(true, Ordering::SeqCst);
    }

    fn fail_remove(&self) {
        self.fail_remove.store(true, Ordering::SeqCst);
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

    fn state(&self, plugin_id: &PluginId, status: PluginStatus) -> PluginRuntimeState {
        PluginRuntimeState {
            plugin_id: plugin_id.clone(),
            status,
            container_id: Some("container-virtual".to_string()),
            logs: vec![PluginRuntimeLog {
                level: "info".to_string(),
                message: "token=secret-token".to_string(),
                context: serde_json::json!({"token": "secret-token"}),
            }],
        }
    }
}

#[async_trait]
impl PluginRuntimeControl for FakeRuntime {
    async fn start(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events.lock().unwrap().push("start");
        if self.block_start.load(Ordering::SeqCst) {
            self.start_entered.notify_waiters();
            self.start_release.notified().await;
        }
        if self.fail_start.swap(false, Ordering::SeqCst) {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        Ok(self.state(plugin_id, PluginStatus::Healthy))
    }

    async fn stop(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events.lock().unwrap().push("stop");
        Ok(self.state(plugin_id, PluginStatus::Stopped))
    }

    async fn inspect(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        self.events.lock().unwrap().push("inspect");
        Ok(self.state(plugin_id, PluginStatus::Healthy))
    }

    async fn remove(&self, plugin_id: &PluginId) -> Result<PluginRemoveResult, PluginManagerError> {
        self.events.lock().unwrap().push("remove");
        if self.fail_remove.swap(false, Ordering::SeqCst) {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
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
