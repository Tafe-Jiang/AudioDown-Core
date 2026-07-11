#[path = "../src/install_operation.rs"]
mod install_operation;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact, PluginInstallOperation,
    PluginInstallOperationState,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use install_operation::{
    BuildAdapterError, BuildOutput, BuildRequest, InstallBuildAdapter, InstallOperationError,
    InstallOperationManager,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

const INSTALLATION_ID: &str = "installation-a";

struct FakeBuildAdapter {
    build_gate: Semaphore,
    build_started: Semaphore,
    fail_next: AtomicBool,
    build_calls: Mutex<Vec<Uuid>>,
    removed_images: Mutex<Vec<String>>,
    cleaned_operations: Mutex<Vec<Uuid>>,
}

impl Default for FakeBuildAdapter {
    fn default() -> Self {
        Self {
            build_gate: Semaphore::new(0),
            build_started: Semaphore::new(0),
            fail_next: AtomicBool::new(false),
            build_calls: Mutex::new(Vec::new()),
            removed_images: Mutex::new(Vec::new()),
            cleaned_operations: Mutex::new(Vec::new()),
        }
    }
}

impl FakeBuildAdapter {
    fn blocked() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn ready() -> Arc<Self> {
        let adapter = Arc::new(Self::default());
        adapter.build_gate.add_permits(32);
        adapter
    }

    async fn wait_until_started(&self) {
        self.build_started.acquire().await.unwrap().forget();
    }

    fn release_one(&self) {
        self.build_gate.add_permits(1);
    }

    fn fail_next_build(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn build_call_count(&self) -> usize {
        self.build_calls.lock().unwrap().len()
    }

    fn removed_images(&self) -> Vec<String> {
        self.removed_images.lock().unwrap().clone()
    }

    fn cleaned_operations(&self) -> Vec<Uuid> {
        self.cleaned_operations.lock().unwrap().clone()
    }
}

#[async_trait]
impl InstallBuildAdapter for FakeBuildAdapter {
    async fn build(&self, request: BuildRequest) -> Result<BuildOutput, BuildAdapterError> {
        assert_eq!(request.installation_id, INSTALLATION_ID);
        assert_eq!(request.plugin_id, plugin_id());
        assert!(request
            .candidate_dir
            .ends_with(request.operation_id.to_string()));
        assert!(request
            .prepared_request
            .ends_with(format!("{}.json", request.operation_id)));
        assert!(request
            .mirrored_grant
            .ends_with(format!("{}.json", request.operation_id)));
        self.build_calls.lock().unwrap().push(request.operation_id);
        self.build_started.add_permits(1);
        self.build_gate.acquire().await.unwrap().forget();

        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(BuildAdapterError::with_logs(
                "BUILD_FAILED",
                vec![PluginBuildLog {
                    sequence: 0,
                    stream: PluginBuildLogStream::System,
                    message: "redacted build failure".to_string(),
                }],
            ));
        }

        Ok(BuildOutput {
            artifact: artifact_for(request.operation_id),
            manifest: br#"{"id":"com.audiodown.virtual.content"}"#.to_vec(),
            base_image_digest:
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    .to_string(),
            sdk_hash: "d".repeat(64),
            build_logs: Vec::new(),
        })
    }

    async fn remove_image(&self, image_id: &str) -> Result<(), BuildAdapterError> {
        self.removed_images
            .lock()
            .unwrap()
            .push(image_id.to_string());
        Ok(())
    }

    async fn cleanup_temporary_resources(
        &self,
        operation_id: Uuid,
    ) -> Result<(), BuildAdapterError> {
        self.cleaned_operations.lock().unwrap().push(operation_id);
        Ok(())
    }
}

#[tokio::test]
async fn begin_is_background_idempotent_and_busy_until_built() {
    assert_eq!(
        BuildAdapterError::new("BUILD_FAILED").code(),
        "BUILD_FAILED"
    );
    let root = TestRoot::new();
    let adapter = FakeBuildAdapter::blocked();
    let manager = manager(&root, adapter.clone()).await;
    let plugin_id = plugin_id();
    let operation_id = Uuid::new_v4();
    create_mirrors(&manager, &plugin_id, operation_id);

    let accepted = manager
        .begin(plugin_id.clone(), operation_id, now())
        .await
        .unwrap();
    assert_eq!(accepted.state, PluginInstallOperationState::Accepted);
    assert!(manager
        .paths(&plugin_id, operation_id)
        .operation_record
        .is_file());
    assert_eq!(manager.list().await.operations.len(), 1);

    adapter.wait_until_started().await;
    let repeated = manager
        .begin(plugin_id.clone(), operation_id, now())
        .await
        .unwrap();
    assert_eq!(repeated.operation_id, operation_id);
    assert_eq!(adapter.build_call_count(), 1);
    assert_eq!(
        manager
            .status(&plugin_id, operation_id)
            .await
            .unwrap()
            .state,
        PluginInstallOperationState::Building
    );

    let second_operation = Uuid::new_v4();
    assert!(matches!(
        manager
            .begin(plugin_id.clone(), second_operation, now())
            .await,
        Err(InstallOperationError::BuildBusy)
    ));

    adapter.release_one();
    let built = wait_for_state(
        &manager,
        &plugin_id,
        operation_id,
        PluginInstallOperationState::Built,
    )
    .await;
    assert!(built.artifact.is_some());
    let paths = manager.paths(&plugin_id, operation_id);
    assert!(paths.candidate_dir.is_dir());
    assert!(!paths.installed_dir.exists());
}

#[tokio::test]
async fn finalize_promotes_candidate_and_retries_cleanup_idempotently() {
    let root = TestRoot::new();
    let adapter = FakeBuildAdapter::ready();
    let manager = manager(&root, adapter).await;
    let plugin_id = plugin_id();
    let operation_id = Uuid::new_v4();
    create_mirrors(&manager, &plugin_id, operation_id);
    let grant_id = Uuid::new_v4();
    let paths = manager.paths(&plugin_id, operation_id);
    write_file(
        &paths.prepared_request,
        serde_json::json!({"riskGrantId": grant_id})
            .to_string()
            .as_bytes(),
    );
    let actual_grant = paths
        .prepared_request
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("grants")
        .join(format!("{grant_id}.json"));
    write_file(&actual_grant, b"grant");
    build_to_state(&manager, &plugin_id, operation_id).await;

    let finalized = manager
        .finalize(&plugin_id, operation_id, now())
        .await
        .unwrap();
    assert_eq!(finalized.state, PluginInstallOperationState::Finalized);
    assert!(!paths.candidate_dir.exists());
    assert!(paths.installed_dir.is_dir());
    assert!(!paths.prepared_request.exists());
    assert!(!paths.mirrored_grant.exists());
    assert!(!actual_grant.exists());

    write_file(&paths.prepared_request, b"prepared");
    write_file(&paths.mirrored_grant, b"grant");
    let repeated = manager
        .finalize(&plugin_id, operation_id, now())
        .await
        .unwrap();
    assert_eq!(repeated, finalized);
    assert!(!paths.prepared_request.exists());
    assert!(!paths.mirrored_grant.exists());
}

#[tokio::test]
async fn finalize_recovers_after_candidate_rename_before_state_persist() {
    let root = TestRoot::new();
    let adapter = FakeBuildAdapter::ready();
    let manager = manager(&root, adapter.clone()).await;
    let plugin_id = plugin_id();
    let operation_id = Uuid::new_v4();
    create_mirrors(&manager, &plugin_id, operation_id);
    build_to_state(&manager, &plugin_id, operation_id).await;
    let paths = manager.paths(&plugin_id, operation_id);
    fs::create_dir_all(paths.installed_dir.parent().unwrap()).unwrap();
    fs::rename(&paths.candidate_dir, &paths.installed_dir).unwrap();
    drop(manager);

    let restarted = InstallOperationManager::open(root.path(), INSTALLATION_ID, adapter, now())
        .await
        .unwrap();
    let finalized = restarted
        .finalize(&plugin_id, operation_id, now())
        .await
        .unwrap();
    assert_eq!(finalized.state, PluginInstallOperationState::Finalized);
    assert!(paths.installed_dir.is_dir());
}

#[tokio::test]
async fn finalize_refuses_to_overwrite_mismatched_installed_attestation() {
    let root = TestRoot::new();
    let adapter = FakeBuildAdapter::ready();
    let manager = manager(&root, adapter).await;
    let plugin_id = plugin_id();
    let operation_id = Uuid::new_v4();
    build_to_state(&manager, &plugin_id, operation_id).await;
    let paths = manager.paths(&plugin_id, operation_id);
    fs::create_dir_all(&paths.installed_dir).unwrap();
    write_file(
        &paths.installed_dir.join("install.json"),
        br#"{"installationId":"another-installation"}"#,
    );

    assert!(matches!(
        manager.finalize(&plugin_id, operation_id, now()).await,
        Err(InstallOperationError::InstalledAttestationMismatch)
    ));
    assert_eq!(
        manager
            .status(&plugin_id, operation_id)
            .await
            .unwrap()
            .state,
        PluginInstallOperationState::Built
    );
    assert!(paths.candidate_dir.is_dir());
    assert!(paths.installed_dir.is_dir());
}

#[tokio::test]
async fn abort_removes_all_operation_owned_resources() {
    let root = TestRoot::new();
    let adapter = FakeBuildAdapter::ready();
    let manager = manager(&root, adapter.clone()).await;
    let plugin_id = plugin_id();
    let operation_id = Uuid::new_v4();
    create_mirrors(&manager, &plugin_id, operation_id);
    let built = build_to_state(&manager, &plugin_id, operation_id).await;
    let image_id = built.artifact.unwrap().image_id;

    let aborted = manager
        .abort(&plugin_id, operation_id, now())
        .await
        .unwrap();
    assert_eq!(aborted.state, PluginInstallOperationState::Aborted);
    let paths = manager.paths(&plugin_id, operation_id);
    assert!(!paths.candidate_dir.exists());
    assert!(!paths.installed_dir.exists());
    assert!(!paths.prepared_request.exists());
    assert!(!paths.mirrored_grant.exists());
    assert_eq!(adapter.removed_images(), vec![image_id]);
    assert!(adapter.cleaned_operations().contains(&operation_id));

    let repeated = manager
        .abort(&plugin_id, operation_id, now())
        .await
        .unwrap();
    assert_eq!(repeated.state, PluginInstallOperationState::Aborted);
}

#[tokio::test]
async fn restart_fails_orphaned_building_and_cleans_temporary_resources() {
    let root = TestRoot::new();
    let adapter = FakeBuildAdapter::blocked();
    let manager = manager(&root, adapter.clone()).await;
    let plugin_id = plugin_id();
    let operation_id = Uuid::new_v4();
    manager
        .begin(plugin_id.clone(), operation_id, now())
        .await
        .unwrap();
    adapter.wait_until_started().await;
    assert_eq!(
        manager
            .status(&plugin_id, operation_id)
            .await
            .unwrap()
            .state,
        PluginInstallOperationState::Building
    );

    let restarted =
        InstallOperationManager::open(root.path(), INSTALLATION_ID, adapter.clone(), now())
            .await
            .unwrap();
    let failed = restarted.status(&plugin_id, operation_id).await.unwrap();
    assert_eq!(failed.state, PluginInstallOperationState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("SUPERVISOR_RESTARTED"));
    assert!(adapter.cleaned_operations().contains(&operation_id));
}

#[tokio::test]
async fn terminal_records_require_ack_and_remain_queryable_for_thirty_minutes() {
    let root = TestRoot::new();
    let adapter = FakeBuildAdapter::ready();
    let manager = manager(&root, adapter.clone()).await;
    let plugin_id = plugin_id();
    let base = now();

    let finalized_id = Uuid::new_v4();
    build_to_state(&manager, &plugin_id, finalized_id).await;
    manager
        .finalize(&plugin_id, finalized_id, base)
        .await
        .unwrap();
    manager
        .acknowledge(&plugin_id, finalized_id, base)
        .await
        .unwrap();

    let aborted_id = Uuid::new_v4();
    build_to_state(&manager, &plugin_id, aborted_id).await;
    manager.abort(&plugin_id, aborted_id, base).await.unwrap();
    manager
        .acknowledge(&plugin_id, aborted_id, base)
        .await
        .unwrap();

    adapter.fail_next_build();
    let failed_id = Uuid::new_v4();
    manager
        .begin(plugin_id.clone(), failed_id, base)
        .await
        .unwrap();
    let failed = wait_for_state(
        &manager,
        &plugin_id,
        failed_id,
        PluginInstallOperationState::Failed,
    )
    .await;
    assert_eq!(failed.build_logs.len(), 1);
    manager
        .acknowledge(&plugin_id, failed_id, base)
        .await
        .unwrap();

    manager
        .cleanup_acknowledged(base + Duration::minutes(29))
        .await
        .unwrap();
    for operation_id in [finalized_id, aborted_id, failed_id] {
        assert!(manager.status(&plugin_id, operation_id).await.is_ok());
    }

    manager
        .cleanup_acknowledged(base + Duration::minutes(31))
        .await
        .unwrap();
    for operation_id in [finalized_id, aborted_id, failed_id] {
        assert!(matches!(
            manager.status(&plugin_id, operation_id).await,
            Err(InstallOperationError::NotFound)
        ));
    }

    let unacknowledged_id = Uuid::new_v4();
    build_to_state(&manager, &plugin_id, unacknowledged_id).await;
    manager
        .abort(&plugin_id, unacknowledged_id, base)
        .await
        .unwrap();
    manager
        .cleanup_acknowledged(base + Duration::days(365))
        .await
        .unwrap();
    assert!(manager.status(&plugin_id, unacknowledged_id).await.is_ok());
}

async fn manager(
    root: &TestRoot,
    adapter: Arc<FakeBuildAdapter>,
) -> InstallOperationManager<FakeBuildAdapter> {
    InstallOperationManager::open(root.path(), INSTALLATION_ID, adapter, now())
        .await
        .unwrap()
}

async fn build_to_state(
    manager: &InstallOperationManager<FakeBuildAdapter>,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> PluginInstallOperation {
    manager
        .begin(plugin_id.clone(), operation_id, now())
        .await
        .unwrap();
    wait_for_state(
        manager,
        plugin_id,
        operation_id,
        PluginInstallOperationState::Built,
    )
    .await
}

async fn wait_for_state(
    manager: &InstallOperationManager<FakeBuildAdapter>,
    plugin_id: &PluginId,
    operation_id: Uuid,
    expected: PluginInstallOperationState,
) -> PluginInstallOperation {
    for _ in 0..200 {
        let operation = manager.status(plugin_id, operation_id).await.unwrap();
        if operation.state == expected {
            return operation;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("operation did not reach {expected:?}");
}

fn create_mirrors(
    manager: &InstallOperationManager<FakeBuildAdapter>,
    plugin_id: &PluginId,
    operation_id: Uuid,
) {
    let paths = manager.paths(plugin_id, operation_id);
    write_file(&paths.prepared_request, b"prepared");
    write_file(&paths.mirrored_grant, b"grant");
}

fn write_file(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn artifact_for(operation_id: Uuid) -> PluginInstallArtifact {
    PluginInstallArtifact {
        image_id: format!("sha256:{operation_id}"),
        repository_id: "virtual.repository".to_string(),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        source_hash: "a".repeat(64),
        manifest_hash: "b".repeat(64),
    }
}

fn plugin_id() -> PluginId {
    PluginId::parse("com.audiodown.virtual.content").unwrap()
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap()
}

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("audiodown-install-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
