use std::{
    collections::{HashMap, VecDeque},
    fs,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus};
use audiodown_plugin_api::manifest::{
    BuildSpec, CompatibilitySpec, LifecycleScriptPolicy, NetworkPolicy, PlatformSpec,
    PluginManifest, PluginType, RuntimeKind, RuntimeSpec,
};
use audiodown_plugin_manager::{
    archive::ExtractedSnapshot,
    github::GitHubRepositoryRef,
    service::{
        InstallError, InstallPluginCommand, InstallPluginRecord, LifecycleAuthorizationError,
        LifecycleRiskAuthorizer, LifecycleRiskInput, PluginBuildLogRecord, PluginManagerService,
        PluginRuntimeControl, PluginStateStore,
    },
    staging::SnapshotStore,
    validation::{ValidatedPlugin, ValidatedRepository},
    PluginManagerError,
};
use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact, PluginInstallOperation,
    PluginInstallOperationList, PluginInstallOperationState, PluginRemoveResult,
    PluginRuntimeState,
};
use chrono::Utc;
use secrecy::SecretString;
use tempfile::TempDir;
use uuid::Uuid;

const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const MANIFEST_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SOURCE_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const IMAGE_ID: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[tokio::test]
async fn installs_staged_plugin_through_two_phase_transaction() {
    let fixture = InstallFixture::new(false).await;

    let installed = fixture.install(false, None).await.unwrap();

    assert_eq!(installed.plugin_id, fixture.plugin_id);
    assert_eq!(installed.status, PluginStatus::Installed);
    assert_eq!(installed.image_id.as_deref(), Some(IMAGE_ID));
    let events = fixture.store.events();
    assert_eq!(
        events,
        ["insert_installing", "build_log", "complete_install"]
    );
    assert_eq!(
        fixture.runtime.events(),
        ["begin", "status", "finalize", "ack"]
    );
    assert_eq!(fixture.store.build_logs().len(), 1);
    assert!(!fixture.store.build_logs()[0]
        .message
        .contains("must-not-leak"));
}

#[tokio::test]
async fn requires_fresh_lifecycle_approval_and_developer_token() {
    let fixture = InstallFixture::new(true).await;

    assert_eq!(
        fixture.install(false, None).await.unwrap_err(),
        InstallError::RiskGrantRequired
    );
    fixture.authorizer.disable_mode();
    assert_eq!(
        fixture
            .install(true, Some(SecretString::from("valid".to_string())))
            .await
            .unwrap_err(),
        InstallError::DeveloperModeRequired
    );
    fixture.authorizer.enable_mode();
    assert_eq!(
        fixture
            .install(true, Some(SecretString::from("wrong".to_string())))
            .await
            .unwrap_err(),
        InstallError::DevTokenRequired
    );
    fixture
        .install(true, Some(SecretString::from("valid".to_string())))
        .await
        .unwrap();

    assert_eq!(fixture.store.grant_count(), 1);
    assert_eq!(fixture.authorizer.calls(), 3);
}

#[tokio::test]
async fn serializes_operations_for_the_same_plugin() {
    let fixture = InstallFixture::new(false).await;
    fixture.runtime.pause_after_begin();

    let first = {
        let service = fixture.service.clone();
        let command = fixture.command(false, None);
        tokio::spawn(async move { service.install(command).await })
    };
    fixture.runtime.wait_until_begun().await;

    assert_eq!(
        fixture.install(false, None).await.unwrap_err(),
        InstallError::PluginOperationInProgress
    );
    fixture.runtime.resume();
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn aborts_and_rolls_back_when_sqlite_insert_fails() {
    let fixture = InstallFixture::new(false).await;
    fixture.store.fail_insert();

    assert_eq!(
        fixture.install(false, None).await.unwrap_err(),
        InstallError::Internal
    );
    assert_eq!(
        fixture.runtime.events(),
        ["begin", "status", "abort", "ack"]
    );
    assert!(fixture.store.records().is_empty());
}

#[tokio::test]
async fn preserves_installing_row_when_finalize_is_uncertain() {
    let fixture = InstallFixture::new(false).await;
    fixture.runtime.fail_finalize();

    assert_eq!(
        fixture.install(false, None).await.unwrap_err(),
        InstallError::RuntimeUnavailable
    );
    let records = fixture.store.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, PluginStatus::Installing);
    assert!(records[0].install_operation_id.is_some());
    assert_eq!(fixture.runtime.events(), ["begin", "status", "finalize"]);
}

#[tokio::test]
async fn reconciles_built_and_finalized_operations_after_restart() {
    let built = InstallFixture::new(false).await;
    let operation_id = Uuid::new_v4();
    built.store.seed_installing(built.record(operation_id));
    built.runtime.seed_operation(operation(
        &built.plugin_id,
        operation_id,
        PluginInstallOperationState::Built,
    ));
    built.service.reconcile_install_operations().await.unwrap();
    assert_eq!(
        built.runtime.events(),
        ["list", "status", "finalize", "ack"]
    );
    assert_eq!(built.store.records()[0].status, PluginStatus::Installed);

    let finalized = InstallFixture::new(false).await;
    let operation_id = Uuid::new_v4();
    finalized
        .store
        .seed_installing(finalized.record(operation_id));
    finalized.runtime.seed_operation(operation(
        &finalized.plugin_id,
        operation_id,
        PluginInstallOperationState::Finalized,
    ));
    finalized
        .service
        .reconcile_install_operations()
        .await
        .unwrap();
    assert_eq!(finalized.runtime.events(), ["list", "status", "ack"]);
    assert_eq!(finalized.store.records()[0].status, PluginStatus::Installed);
}

#[tokio::test]
async fn rejects_missing_expired_and_unlisted_staged_plugins() {
    let fixture = InstallFixture::new(false).await;
    let missing = InstallPluginCommand {
        snapshot_id: Uuid::new_v4(),
        plugin_id: fixture.plugin_id.clone(),
        lifecycle_risk: LifecycleRiskInput {
            explicitly_approved: false,
            developer_token: None,
        },
    };
    assert_eq!(
        fixture.service.install(missing).await.unwrap_err(),
        InstallError::SnapshotNotFound
    );

    let unlisted = InstallPluginCommand {
        snapshot_id: fixture.snapshot_id,
        plugin_id: PluginId::parse("com.audiodown.virtual.unlisted").unwrap(),
        lifecycle_risk: LifecycleRiskInput {
            explicitly_approved: false,
            developer_token: None,
        },
    };
    assert_eq!(
        fixture.service.install(unlisted).await.unwrap_err(),
        InstallError::PluginNotInSnapshot
    );

    let metadata_path = fixture
        .plugin_data
        .join("staging")
        .join(fixture.snapshot_id.to_string())
        .join("snapshot.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["createdAt"] = serde_json::json!(Utc::now() - chrono::Duration::minutes(31));
    fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    assert_eq!(
        fixture.install(false, None).await.unwrap_err(),
        InstallError::SnapshotNotFound
    );
}

#[tokio::test]
async fn distinguishes_existing_installed_and_installing_rows() {
    let installed = InstallFixture::new(false).await;
    let mut record = installed.record(Uuid::new_v4());
    record.status = PluginStatus::Installed;
    record.install_operation_id = None;
    installed.store.seed_installing(record);
    assert_eq!(
        installed.install(false, None).await.unwrap_err(),
        InstallError::PluginAlreadyInstalled
    );

    let installing = InstallFixture::new(false).await;
    installing
        .store
        .seed_installing(installing.record(Uuid::new_v4()));
    assert_eq!(
        installing.install(false, None).await.unwrap_err(),
        InstallError::PluginOperationInProgress
    );
}

#[tokio::test]
async fn rejects_mismatched_artifacts_and_persists_failed_build_logs() {
    let mismatch = InstallFixture::new(false).await;
    let mut mismatched = operation(
        &mismatch.plugin_id,
        Uuid::new_v4(),
        PluginInstallOperationState::Built,
    );
    mismatched.artifact.as_mut().unwrap().source_hash = "d".repeat(64);
    mismatch.runtime.seed_operation(mismatched);
    assert_eq!(
        mismatch.install(false, None).await.unwrap_err(),
        InstallError::ArtifactMismatch
    );
    assert_eq!(mismatch.runtime.events(), ["begin", "abort", "ack"]);
    assert!(mismatch.store.records().is_empty());

    let failed = InstallFixture::new(false).await;
    failed.runtime.seed_operation(operation(
        &failed.plugin_id,
        Uuid::new_v4(),
        PluginInstallOperationState::Failed,
    ));
    assert_eq!(
        failed.install(false, None).await.unwrap_err(),
        InstallError::BuildFailed
    );
    assert_eq!(failed.runtime.events(), ["begin", "abort", "ack"]);
    assert_eq!(failed.store.build_logs().len(), 1);
    assert!(!failed.store.build_logs()[0]
        .message
        .contains("must-not-leak"));
}

#[tokio::test]
async fn recovers_an_ambiguous_begin_with_the_same_operation_id() {
    let fixture = InstallFixture::new(false).await;
    fixture.runtime.fail_begin_once();
    fixture.runtime.fail_status_times(2);

    fixture.install(false, None).await.unwrap();

    assert_eq!(
        fixture.runtime.events(),
        ["begin", "status", "status", "status", "finalize", "ack"]
    );
}

#[tokio::test]
async fn times_out_the_http_wait_and_attempts_abort() {
    let fixture = InstallFixture::new(false).await;
    fixture.runtime.hold_build();

    assert_eq!(
        fixture.install(false, None).await.unwrap_err(),
        InstallError::InstallTimeout
    );
    let events = fixture.runtime.events();
    assert_eq!(events.first().map(String::as_str), Some("begin"));
    assert_eq!(&events[events.len() - 2..], ["abort", "ack"]);
}

#[tokio::test]
async fn historical_risk_audit_never_skips_fresh_authorization() {
    let fixture = InstallFixture::new(true).await;
    fixture.store.fail_insert();
    assert_eq!(
        fixture
            .install(true, Some(SecretString::from("valid".to_string())))
            .await
            .unwrap_err(),
        InstallError::Internal
    );
    fixture.store.clear_fail_insert();
    fixture
        .install(true, Some(SecretString::from("valid".to_string())))
        .await
        .unwrap();

    assert_eq!(fixture.authorizer.calls(), 2);
    assert_eq!(fixture.store.grant_count(), 2);
}

#[tokio::test]
async fn build_log_storage_failure_never_prevents_runtime_cleanup() {
    let fixture = InstallFixture::new(false).await;
    fixture.store.fail_build_logs();
    fixture.runtime.seed_operation(operation(
        &fixture.plugin_id,
        Uuid::new_v4(),
        PluginInstallOperationState::Failed,
    ));

    assert_eq!(
        fixture.install(false, None).await.unwrap_err(),
        InstallError::Internal
    );
    assert_eq!(fixture.runtime.events(), ["begin", "abort", "ack"]);
}

#[tokio::test]
async fn reconciles_orphaned_and_failed_operations_without_touching_installed_assets() {
    let orphaned_built = InstallFixture::new(false).await;
    let operation_id = Uuid::new_v4();
    orphaned_built.runtime.seed_operation(operation(
        &orphaned_built.plugin_id,
        operation_id,
        PluginInstallOperationState::Built,
    ));
    orphaned_built
        .service
        .reconcile_install_operations()
        .await
        .unwrap();
    assert_eq!(
        orphaned_built.runtime.events(),
        ["list", "status", "abort", "ack"]
    );

    let orphaned_finalized = InstallFixture::new(false).await;
    let operation_id = Uuid::new_v4();
    orphaned_finalized.runtime.seed_operation(operation(
        &orphaned_finalized.plugin_id,
        operation_id,
        PluginInstallOperationState::Finalized,
    ));
    orphaned_finalized
        .service
        .reconcile_install_operations()
        .await
        .unwrap();
    assert_eq!(
        orphaned_finalized.runtime.events(),
        ["list", "status", "abort", "ack"]
    );

    let installed = InstallFixture::new(false).await;
    let operation_id = Uuid::new_v4();
    let mut record = installed.record(operation_id);
    record.status = PluginStatus::Installed;
    record.install_operation_id = None;
    installed.store.seed_installing(record);
    installed.runtime.seed_operation(operation(
        &installed.plugin_id,
        operation_id,
        PluginInstallOperationState::Finalized,
    ));
    installed
        .service
        .reconcile_install_operations()
        .await
        .unwrap();
    assert_eq!(installed.runtime.events(), ["list", "status", "ack"]);
}

#[tokio::test]
async fn never_completes_a_finalized_operation_with_mismatched_artifacts() {
    let fixture = InstallFixture::new(false).await;
    let operation_id = Uuid::new_v4();
    fixture.store.seed_installing(fixture.record(operation_id));
    let mut finalized = operation(
        &fixture.plugin_id,
        operation_id,
        PluginInstallOperationState::Finalized,
    );
    finalized.artifact.as_mut().unwrap().manifest_hash = "d".repeat(64);
    fixture.runtime.seed_operation(finalized);

    assert_eq!(
        fixture
            .service
            .reconcile_install_operations()
            .await
            .unwrap_err(),
        InstallError::ArtifactMismatch
    );
    assert_eq!(fixture.runtime.events(), ["list", "status"]);
    assert_eq!(fixture.store.records()[0].status, PluginStatus::Installing);
}

#[tokio::test]
async fn recovers_sqlite_only_pending_rows_by_the_original_operation_id() {
    let fixture = InstallFixture::new(false).await;
    let operation_id = Uuid::new_v4();
    fixture.store.seed_installing(fixture.record(operation_id));
    fixture.runtime.omit_operations_from_list();
    fixture.runtime.seed_operation(operation(
        &fixture.plugin_id,
        operation_id,
        PluginInstallOperationState::Built,
    ));

    fixture
        .service
        .reconcile_install_operations()
        .await
        .unwrap();

    assert_eq!(
        fixture.runtime.events(),
        ["list", "status", "finalize", "ack"]
    );
    assert_eq!(fixture.store.records()[0].status, PluginStatus::Installed);
}

struct InstallFixture {
    _temp: TempDir,
    plugin_data: std::path::PathBuf,
    plugin_id: PluginId,
    snapshot_id: Uuid,
    service: Arc<PluginManagerService>,
    store: Arc<FakeStore>,
    runtime: Arc<FakeRuntime>,
    authorizer: Arc<FakeAuthorizer>,
}

impl InstallFixture {
    async fn new(requires_scripts: bool) -> Self {
        let temp = TempDir::new().unwrap();
        let plugin_data = temp.path().join("plugins");
        let snapshot_store = SnapshotStore::new(&plugin_data);
        let extracted_root = temp.path().join("repository");
        let plugin_root = extracted_root.join("plugins/virtual-content");
        fs::create_dir_all(plugin_root.join("src")).unwrap();
        fs::write(
            plugin_root.join("audiodown-plugin.json"),
            serde_json::to_vec(&manifest(requires_scripts)).unwrap(),
        )
        .unwrap();
        fs::write(
            plugin_root.join("package.json"),
            r#"{"name":"virtual-content","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("package-lock.json"),
            r#"{"name":"virtual-content","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"virtual-content","version":"1.0.0"}}}"#,
        )
        .unwrap();
        fs::write(plugin_root.join("src/index.js"), "export default {};\n").unwrap();
        let preview = snapshot_store
            .create(
                &GitHubRepositoryRef::parse("https://github.com/example-owner/example-repository")
                    .unwrap(),
                COMMIT_SHA,
                ExtractedSnapshot {
                    repository_root: extracted_root,
                    file_count: 4,
                    extracted_bytes: 256,
                },
                validated_repository(requires_scripts),
            )
            .await
            .unwrap();

        let store = Arc::new(FakeStore::default());
        let runtime = Arc::new(FakeRuntime::default());
        let authorizer = Arc::new(FakeAuthorizer::default());
        let service = Arc::new(
            PluginManagerService::new(
                store.clone(),
                Arc::new(UnusedRepositorySource),
                plugin_data.clone(),
                "1.0.0-alpha.1".parse().unwrap(),
                "1.0.0".parse().unwrap(),
            )
            .with_installation_ports(runtime.clone(), authorizer.clone())
            .with_install_timing(Duration::from_millis(1), Duration::from_millis(20)),
        );

        Self {
            _temp: temp,
            plugin_data,
            plugin_id: PluginId::parse("com.audiodown.virtual.content").unwrap(),
            snapshot_id: preview.snapshot_id,
            service,
            store,
            runtime,
            authorizer,
        }
    }

    fn command(&self, approved: bool, token: Option<SecretString>) -> InstallPluginCommand {
        InstallPluginCommand {
            snapshot_id: self.snapshot_id,
            plugin_id: self.plugin_id.clone(),
            lifecycle_risk: LifecycleRiskInput {
                explicitly_approved: approved,
                developer_token: token,
            },
        }
    }

    async fn install(
        &self,
        approved: bool,
        token: Option<SecretString>,
    ) -> Result<InstallPluginRecord, InstallError> {
        self.service.install(self.command(approved, token)).await
    }

    fn record(&self, operation_id: Uuid) -> InstallPluginRecord {
        InstallPluginRecord {
            operation_id,
            plugin_id: self.plugin_id.clone(),
            plugin_type: PluginType::Content,
            platform_id: "virtual".to_string(),
            name: "Virtual Content".to_string(),
            version: "1.0.0".to_string(),
            protocol_version: "1.0".to_string(),
            source_ref: "https://github.com/example-owner/example-repository".to_string(),
            commit_sha: COMMIT_SHA.to_string(),
            repository_id: "example.plugins".to_string(),
            manifest_json: serde_json::to_value(manifest(false)).unwrap(),
            manifest_hash: MANIFEST_HASH.to_string(),
            source_hash: SOURCE_HASH.to_string(),
            image_id: Some(IMAGE_ID.to_string()),
            status: PluginStatus::Installing,
            install_operation_id: Some(operation_id),
            installed_at: Utc::now(),
        }
    }
}

#[derive(Default)]
struct FakeStore {
    records: Mutex<HashMap<PluginId, InstallPluginRecord>>,
    grants: Mutex<usize>,
    events: Mutex<Vec<String>>,
    fail_insert: Mutex<bool>,
    fail_logs: Mutex<bool>,
    build_logs: Mutex<Vec<PluginBuildLogRecord>>,
}

impl FakeStore {
    fn records(&self) -> Vec<InstallPluginRecord> {
        self.records.lock().unwrap().values().cloned().collect()
    }

    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn grant_count(&self) -> usize {
        *self.grants.lock().unwrap()
    }

    fn fail_insert(&self) {
        *self.fail_insert.lock().unwrap() = true;
    }

    fn clear_fail_insert(&self) {
        *self.fail_insert.lock().unwrap() = false;
    }

    fn fail_build_logs(&self) {
        *self.fail_logs.lock().unwrap() = true;
    }

    fn build_logs(&self) -> Vec<PluginBuildLogRecord> {
        self.build_logs.lock().unwrap().clone()
    }

    fn seed_installing(&self, record: InstallPluginRecord) {
        self.records
            .lock()
            .unwrap()
            .insert(record.plugin_id.clone(), record);
    }
}

#[async_trait]
impl PluginStateStore for FakeStore {
    async fn is_installed(&self, plugin_id: &PluginId) -> Result<bool, PluginManagerError> {
        Ok(self.records.lock().unwrap().contains_key(plugin_id))
    }

    async fn persist_risk_grant(
        &self,
        _grant: &audiodown_plugin_manager::staging::LifecycleRiskGrant,
    ) -> Result<(), PluginManagerError> {
        *self.grants.lock().unwrap() += 1;
        Ok(())
    }

    async fn insert_installing(
        &self,
        record: &InstallPluginRecord,
    ) -> Result<(), PluginManagerError> {
        self.events.lock().unwrap().push("insert_installing".into());
        if *self.fail_insert.lock().unwrap() {
            return Err(PluginManagerError::PluginStateUnavailable);
        }
        self.records
            .lock()
            .unwrap()
            .insert(record.plugin_id.clone(), record.clone());
        Ok(())
    }

    async fn complete_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<InstallPluginRecord, PluginManagerError> {
        self.events.lock().unwrap().push("complete_install".into());
        let mut records = self.records.lock().unwrap();
        let record = records.get_mut(plugin_id).unwrap();
        assert_eq!(record.install_operation_id, Some(operation_id));
        record.status = PluginStatus::Installed;
        record.install_operation_id = None;
        Ok(record.clone())
    }

    async fn rollback_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<(), PluginManagerError> {
        self.events.lock().unwrap().push("rollback_install".into());
        let mut records = self.records.lock().unwrap();
        if records
            .get(plugin_id)
            .is_some_and(|record| record.install_operation_id == Some(operation_id))
        {
            records.remove(plugin_id);
        }
        Ok(())
    }

    async fn list_install_records(&self) -> Result<Vec<InstallPluginRecord>, PluginManagerError> {
        Ok(self.records())
    }

    async fn persist_build_log(
        &self,
        record: &PluginBuildLogRecord,
    ) -> Result<(), PluginManagerError> {
        self.events.lock().unwrap().push("build_log".into());
        if *self.fail_logs.lock().unwrap() {
            return Err(PluginManagerError::PluginStateUnavailable);
        }
        self.build_logs.lock().unwrap().push(record.clone());
        Ok(())
    }
}

#[derive(Default)]
struct FakeRuntime {
    events: Mutex<Vec<String>>,
    operations: Mutex<VecDeque<PluginInstallOperation>>,
    fail_finalize: Mutex<bool>,
    pause: tokio::sync::Notify,
    resumed: tokio::sync::Notify,
    should_pause: Mutex<bool>,
    fail_begin: Mutex<bool>,
    fail_status_count: Mutex<usize>,
    hold_build: Mutex<bool>,
    omit_list: Mutex<bool>,
}

impl FakeRuntime {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn fail_finalize(&self) {
        *self.fail_finalize.lock().unwrap() = true;
    }

    fn fail_begin_once(&self) {
        *self.fail_begin.lock().unwrap() = true;
    }

    fn fail_status_times(&self, count: usize) {
        *self.fail_status_count.lock().unwrap() = count;
    }

    fn hold_build(&self) {
        *self.hold_build.lock().unwrap() = true;
    }

    fn omit_operations_from_list(&self) {
        *self.omit_list.lock().unwrap() = true;
    }

    fn seed_operation(&self, operation: PluginInstallOperation) {
        self.operations.lock().unwrap().push_back(operation);
    }

    fn pause_after_begin(&self) {
        *self.should_pause.lock().unwrap() = true;
    }

    async fn wait_until_begun(&self) {
        self.pause.notified().await;
    }

    fn resume(&self) {
        self.resumed.notify_waiters();
    }

    fn next_or(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        state: PluginInstallOperationState,
    ) -> PluginInstallOperation {
        let mut operation = self
            .operations
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| operation(plugin_id, operation_id, state));
        operation.plugin_id = plugin_id.clone();
        operation.operation_id = operation_id;
        operation
    }
}

#[async_trait]
impl PluginRuntimeControl for FakeRuntime {
    async fn start(&self, _plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        unimplemented!()
    }

    async fn stop(&self, _plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        unimplemented!()
    }

    async fn inspect(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        unimplemented!()
    }

    async fn remove(&self, plugin_id: &PluginId) -> Result<PluginRemoveResult, PluginManagerError> {
        self.events.lock().unwrap().push("remove".into());
        Ok(PluginRemoveResult {
            plugin_id: plugin_id.clone(),
            removed_container: false,
            removed_image: true,
            removed_install_directory: true,
        })
    }

    async fn begin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.events.lock().unwrap().push("begin".into());
        if std::mem::take(&mut *self.fail_begin.lock().unwrap()) {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        if *self.should_pause.lock().unwrap() {
            self.pause.notify_waiters();
            self.resumed.notified().await;
        }
        Ok(self.next_or(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Accepted,
        ))
    }

    async fn install_status(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.events.lock().unwrap().push("status".into());
        let mut failures = self.fail_status_count.lock().unwrap();
        if *failures > 0 {
            *failures -= 1;
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        drop(failures);
        if *self.hold_build.lock().unwrap() {
            return Ok(operation(
                plugin_id,
                operation_id,
                PluginInstallOperationState::Accepted,
            ));
        }
        Ok(self.next_or(plugin_id, operation_id, PluginInstallOperationState::Built))
    }

    async fn finalize_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.events.lock().unwrap().push("finalize".into());
        if *self.fail_finalize.lock().unwrap() {
            return Err(PluginManagerError::RuntimeUnavailable);
        }
        Ok(self.next_or(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Finalized,
        ))
    }

    async fn abort_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.events.lock().unwrap().push("abort".into());
        Ok(self.next_or(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Aborted,
        ))
    }

    async fn list_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, PluginManagerError> {
        self.events.lock().unwrap().push("list".into());
        if *self.omit_list.lock().unwrap() {
            return Ok(PluginInstallOperationList::new(Vec::new()));
        }
        let operations = self
            .operations
            .lock()
            .unwrap()
            .iter()
            .map(PluginInstallOperation::summary)
            .collect();
        Ok(PluginInstallOperationList::new(operations))
    }

    async fn acknowledge_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.events.lock().unwrap().push("ack".into());
        let mut operation = self.next_or(
            plugin_id,
            operation_id,
            PluginInstallOperationState::Finalized,
        );
        operation.acknowledged = true;
        Ok(operation)
    }
}

struct FakeAuthorizer {
    calls: Mutex<usize>,
    mode_enabled: Mutex<bool>,
}

impl FakeAuthorizer {
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }

    fn disable_mode(&self) {
        *self.mode_enabled.lock().unwrap() = false;
    }

    fn enable_mode(&self) {
        *self.mode_enabled.lock().unwrap() = true;
    }
}

#[async_trait]
impl LifecycleRiskAuthorizer for FakeAuthorizer {
    async fn authorize(
        &self,
        token: Option<&SecretString>,
    ) -> Result<(), LifecycleAuthorizationError> {
        *self.calls.lock().unwrap() += 1;
        if !*self.mode_enabled.lock().unwrap() {
            return Err(LifecycleAuthorizationError::DeveloperModeRequired);
        }
        match token
            .map(secrecy::ExposeSecret::expose_secret)
            .map(String::as_str)
        {
            Some("valid") => Ok(()),
            _ => Err(LifecycleAuthorizationError::TokenRequired),
        }
    }
}

impl Default for FakeAuthorizer {
    fn default() -> Self {
        Self {
            calls: Mutex::new(0),
            mode_enabled: Mutex::new(true),
        }
    }
}

struct UnusedRepositorySource;

#[async_trait]
impl audiodown_plugin_manager::RepositorySource for UnusedRepositorySource {
    async fn resolve_and_download(
        &self,
        _source: &GitHubRepositoryRef,
        _destination: &std::path::Path,
    ) -> Result<audiodown_plugin_manager::DownloadedSnapshot, PluginManagerError> {
        unimplemented!()
    }
}

fn operation(
    plugin_id: &PluginId,
    operation_id: Uuid,
    state: PluginInstallOperationState,
) -> PluginInstallOperation {
    PluginInstallOperation {
        operation_id,
        plugin_id: plugin_id.clone(),
        state,
        artifact: matches!(
            state,
            PluginInstallOperationState::Built | PluginInstallOperationState::Finalized
        )
        .then(|| PluginInstallArtifact {
            image_id: IMAGE_ID.to_string(),
            repository_id: "example.plugins".to_string(),
            commit_sha: COMMIT_SHA.to_string(),
            source_hash: SOURCE_HASH.to_string(),
            manifest_hash: MANIFEST_HASH.to_string(),
        }),
        build_logs: vec![PluginBuildLog {
            sequence: 1,
            stream: PluginBuildLogStream::Stdout,
            message: "build token=must-not-leak".to_string(),
        }],
        error_code: None,
        acknowledged: false,
    }
}

fn validated_repository(requires_scripts: bool) -> ValidatedRepository {
    ValidatedRepository {
        repository_id: "example.plugins".to_string(),
        repository_name: "Example Plugins".to_string(),
        plugins: vec![ValidatedPlugin {
            relative_path: "plugins/virtual-content".to_string(),
            manifest: manifest(requires_scripts),
            manifest_hash: MANIFEST_HASH.to_string(),
            source_hash: SOURCE_HASH.to_string(),
            entry_path: "src/index.js".to_string(),
            requires_lifecycle_scripts: requires_scripts,
            lifecycle_script_reason: requires_scripts
                .then(|| "Generate a deterministic local file".to_string()),
        }],
    }
}

fn manifest(requires_scripts: bool) -> PluginManifest {
    PluginManifest {
        schema_version: "1.0".to_string(),
        id: PluginId::parse("com.audiodown.virtual.content").unwrap(),
        name: "Virtual Content".to_string(),
        version: "1.0.0".parse().unwrap(),
        plugin_type: PluginType::Content,
        runtime: RuntimeSpec {
            kind: RuntimeKind::Nodejs,
            version: "22".to_string(),
            entry: "src/index.js".to_string(),
        },
        compatibility: CompatibilitySpec {
            plugin_api: ">=1.0 <2.0".to_string(),
            core: ">=1.0 <2.0".to_string(),
        },
        platform: PlatformSpec {
            id: "virtual".to_string(),
            name: "Virtual".to_string(),
        },
        capabilities: vec!["content.search".to_string()],
        network: NetworkPolicy {
            allowed_hosts: Vec::new(),
        },
        build: BuildSpec {
            npm_lifecycle_scripts: LifecycleScriptPolicy {
                required: requires_scripts,
                reason: requires_scripts.then(|| "Generate a deterministic local file".to_string()),
            },
        },
    }
}
