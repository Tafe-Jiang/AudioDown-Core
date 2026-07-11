use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginInstallArtifact, PluginInstallOperation, PluginInstallOperationList,
    PluginInstallOperationState,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

const ACKNOWLEDGED_RETENTION: Duration = Duration::minutes(30);
const MAX_UNACKNOWLEDGED_OPERATIONS: usize = 256;
const RESTART_ERROR_CODE: &str = "SUPERVISOR_RESTARTED";

#[derive(Debug, Clone)]
pub struct BuildRequest {
    pub installation_id: String,
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
    pub candidate_dir: PathBuf,
    pub prepared_request: PathBuf,
    pub mirrored_grant: PathBuf,
}

#[derive(Debug, Clone)]
pub struct BuildOutput {
    pub artifact: PluginInstallArtifact,
    pub manifest: Vec<u8>,
    pub base_image_digest: String,
    pub sdk_hash: String,
    pub build_logs: Vec<PluginBuildLog>,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{code}")]
pub struct BuildAdapterError {
    code: String,
    build_logs: Vec<PluginBuildLog>,
}

impl BuildAdapterError {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            build_logs: Vec::new(),
        }
    }

    pub fn with_logs(code: impl Into<String>, build_logs: Vec<PluginBuildLog>) -> Self {
        Self {
            code: code.into(),
            build_logs,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn build_logs(&self) -> &[PluginBuildLog] {
        &self.build_logs
    }
}

#[async_trait]
pub trait InstallBuildAdapter: Send + Sync + 'static {
    async fn build(&self, request: BuildRequest) -> Result<BuildOutput, BuildAdapterError>;

    async fn remove_image(&self, image_id: &str) -> Result<(), BuildAdapterError>;

    async fn cleanup_temporary_resources(
        &self,
        operation_id: Uuid,
    ) -> Result<(), BuildAdapterError>;
}

pub struct InstallOperationManager<A> {
    inner: Arc<Inner<A>>,
}

impl<A> Clone for InstallOperationManager<A> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct Inner<A> {
    root: PathBuf,
    installation_id: String,
    adapter: Arc<A>,
    state: Mutex<OperationState>,
}

#[derive(Default)]
struct OperationState {
    operations: BTreeMap<Uuid, StoredOperation>,
    next_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredOperation {
    installation_id: String,
    operation: PluginInstallOperation,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallAttestation {
    installation_id: String,
    operation_id: Uuid,
    plugin_id: PluginId,
    image_id: String,
    source_kind: String,
    repository_id: String,
    commit_sha: String,
    source_hash: String,
    manifest_hash: String,
    base_image_digest: String,
    sdk_hash: String,
    memory_bytes: i64,
    nano_cpus: i64,
    pids_limit: i64,
    run_mode: audiodown_domain::plugin::RunMode,
}

#[derive(Debug, Clone)]
pub struct InstallOperationPaths {
    pub operation_record: PathBuf,
    pub candidate_dir: PathBuf,
    pub installed_dir: PathBuf,
    pub prepared_request: PathBuf,
    pub mirrored_grant: PathBuf,
}

impl<A> InstallOperationManager<A>
where
    A: InstallBuildAdapter,
{
    pub async fn open(
        root: impl AsRef<Path>,
        installation_id: impl Into<String>,
        adapter: Arc<A>,
        now: DateTime<Utc>,
    ) -> Result<Self, InstallOperationError> {
        let root = root.as_ref().to_path_buf();
        ensure_layout(&root)?;
        let state = load_state(&root)?;
        let manager = Self {
            inner: Arc::new(Inner {
                root,
                installation_id: installation_id.into(),
                adapter,
                state: Mutex::new(state),
            }),
        };
        manager.reconcile_restart(now).await?;
        manager.cleanup_acknowledged(now).await?;
        Ok(manager)
    }

    pub fn paths(&self, plugin_id: &PluginId, operation_id: Uuid) -> InstallOperationPaths {
        paths(&self.inner.root, plugin_id, operation_id)
    }

    pub async fn begin(
        &self,
        plugin_id: PluginId,
        operation_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let operation = {
            let mut state = self.inner.state.lock().await;
            if let Some(existing) = state.operations.get(&operation_id) {
                if existing.operation.plugin_id != plugin_id {
                    return Err(InstallOperationError::OperationIdMismatch);
                }
                return Ok(existing.operation.clone());
            }
            if state
                .operations
                .values()
                .any(|stored| !stored.operation.state.is_terminal())
            {
                return Err(InstallOperationError::BuildBusy);
            }
            let unacknowledged = state
                .operations
                .values()
                .filter(|stored| !stored.operation.acknowledged)
                .count();
            if unacknowledged >= MAX_UNACKNOWLEDGED_OPERATIONS {
                return Err(InstallOperationError::OperationCapacityReached);
            }

            let operation = PluginInstallOperation {
                operation_id,
                plugin_id: plugin_id.clone(),
                state: PluginInstallOperationState::Accepted,
                artifact: None,
                build_logs: Vec::new(),
                error_code: None,
                acknowledged: false,
            };
            let sequence = state.next_sequence;
            state.next_sequence = state.next_sequence.saturating_add(1);
            let stored = StoredOperation {
                installation_id: self.inner.installation_id.clone(),
                operation: operation.clone(),
                created_at: now,
                updated_at: now,
                sequence,
            };
            persist_record(&self.inner.root, &stored)?;
            state.operations.insert(operation_id, stored);
            operation
        };

        let manager = self.clone();
        tokio::spawn(async move {
            manager.run_build(plugin_id, operation_id).await;
        });
        Ok(operation)
    }

    pub async fn status(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let state = self.inner.state.lock().await;
        matching_operation(&state, plugin_id, operation_id).map(|stored| stored.operation.clone())
    }

    pub async fn list(&self) -> PluginInstallOperationList {
        let state = self.inner.state.lock().await;
        let mut operations = state
            .operations
            .values()
            .filter(|stored| {
                stored.installation_id == self.inner.installation_id
                    && !stored.operation.acknowledged
            })
            .collect::<Vec<_>>();
        operations.sort_by_key(|stored| stored.sequence);
        PluginInstallOperationList::new(
            operations
                .into_iter()
                .map(|stored| stored.operation.summary())
                .collect(),
        )
    }

    pub async fn finalize(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let operation = self.status(plugin_id, operation_id).await?;
        let artifact = operation
            .artifact
            .clone()
            .ok_or(InstallOperationError::InvalidTransition)?;
        let operation_paths = self.paths(plugin_id, operation_id);
        let attestation_directory = if operation_paths.installed_dir.exists() {
            &operation_paths.installed_dir
        } else {
            &operation_paths.candidate_dir
        };
        let expected = self.attestation_from_directory(
            attestation_directory,
            plugin_id,
            operation_id,
            artifact,
        )?;

        match operation.state {
            PluginInstallOperationState::Finalized => {
                verify_attestation(&operation_paths.installed_dir, &expected)?;
                remove_mirrors(&self.inner.root, &operation_paths)?;
                return Ok(operation);
            }
            PluginInstallOperationState::Built => {}
            _ => return Err(InstallOperationError::InvalidTransition),
        }

        if operation_paths.installed_dir.exists() {
            verify_attestation(&operation_paths.installed_dir, &expected)?;
            if operation_paths.candidate_dir.exists() {
                return Err(InstallOperationError::InstalledAttestationMismatch);
            }
        } else {
            verify_attestation(&operation_paths.candidate_dir, &expected)?;
            let installed_parent = operation_paths
                .installed_dir
                .parent()
                .ok_or(InstallOperationError::InvalidPath)?
                .to_path_buf();
            fs::create_dir_all(&installed_parent)
                .map_err(|error| io_error(&installed_parent, error))?;
            fs::rename(
                &operation_paths.candidate_dir,
                &operation_paths.installed_dir,
            )
            .map_err(|error| io_error(&operation_paths.candidate_dir, error))?;
            sync_directory(&installed_parent)?;
        }

        let finalized = self
            .set_state(
                plugin_id,
                operation_id,
                PluginInstallOperationState::Finalized,
                None,
                now,
            )
            .await?;
        remove_mirrors(&self.inner.root, &operation_paths)?;
        Ok(finalized)
    }

    pub async fn abort(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let operation = self.status(plugin_id, operation_id).await?;
        if operation.state == PluginInstallOperationState::Finalized {
            let artifact = operation
                .artifact
                .clone()
                .ok_or(InstallOperationError::InvalidTransition)?;
            let installed_dir = self.paths(plugin_id, operation_id).installed_dir;
            let expected =
                self.attestation_from_directory(&installed_dir, plugin_id, operation_id, artifact)?;
            verify_attestation(
                &self.paths(plugin_id, operation_id).installed_dir,
                &expected,
            )?;
        }

        let aborted = if operation.state == PluginInstallOperationState::Aborted {
            operation.clone()
        } else {
            self.set_state(
                plugin_id,
                operation_id,
                PluginInstallOperationState::Aborted,
                operation.error_code.clone(),
                now,
            )
            .await?
        };

        let operation_paths = self.paths(plugin_id, operation_id);
        remove_directory(&operation_paths.candidate_dir)?;
        if operation.state == PluginInstallOperationState::Finalized
            && operation_paths.installed_dir.exists()
        {
            let artifact = operation
                .artifact
                .clone()
                .ok_or(InstallOperationError::InvalidTransition)?;
            let expected = self.attestation_from_directory(
                &operation_paths.installed_dir,
                plugin_id,
                operation_id,
                artifact,
            )?;
            verify_attestation(&operation_paths.installed_dir, &expected)?;
            remove_directory(&operation_paths.installed_dir)?;
        }
        remove_mirrors(&self.inner.root, &operation_paths)?;
        if let Some(artifact) = operation.artifact {
            self.inner.adapter.remove_image(&artifact.image_id).await?;
        }
        self.inner
            .adapter
            .cleanup_temporary_resources(operation_id)
            .await?;
        Ok(aborted)
    }

    pub async fn acknowledge(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let mut state = self.inner.state.lock().await;
        let stored = matching_operation_mut(&mut state, plugin_id, operation_id)?;
        if !stored.operation.state.is_terminal() {
            return Err(InstallOperationError::NotTerminal);
        }
        stored.operation.acknowledged = true;
        stored.updated_at = now;
        persist_record(&self.inner.root, stored)?;
        Ok(stored.operation.clone())
    }

    pub async fn cleanup_acknowledged(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(), InstallOperationError> {
        let expired = {
            let state = self.inner.state.lock().await;
            state
                .operations
                .iter()
                .filter(|(_, stored)| {
                    stored.operation.acknowledged
                        && stored.operation.state.is_terminal()
                        && now.signed_duration_since(stored.updated_at) >= ACKNOWLEDGED_RETENTION
                })
                .map(|(operation_id, _)| *operation_id)
                .collect::<Vec<_>>()
        };

        let mut state = self.inner.state.lock().await;
        for operation_id in expired {
            remove_file_if_exists(&operation_record_path(&self.inner.root, operation_id))?;
            state.operations.remove(&operation_id);
        }
        Ok(())
    }

    async fn run_build(&self, plugin_id: PluginId, operation_id: Uuid) {
        if self
            .set_state(
                &plugin_id,
                operation_id,
                PluginInstallOperationState::Building,
                None,
                Utc::now(),
            )
            .await
            .is_err()
        {
            return;
        }

        let operation_paths = self.paths(&plugin_id, operation_id);
        let request = BuildRequest {
            installation_id: self.inner.installation_id.clone(),
            operation_id,
            plugin_id: plugin_id.clone(),
            candidate_dir: operation_paths.candidate_dir.clone(),
            prepared_request: operation_paths.prepared_request.clone(),
            mirrored_grant: operation_paths.mirrored_grant.clone(),
        };
        let result = self.inner.adapter.build(request).await;

        match result {
            Ok(output) => {
                let still_building = self
                    .status(&plugin_id, operation_id)
                    .await
                    .map(|operation| operation.state == PluginInstallOperationState::Building)
                    .unwrap_or(false);
                if !still_building {
                    let _ = self
                        .inner
                        .adapter
                        .remove_image(&output.artifact.image_id)
                        .await;
                } else {
                    let attestation = self.attestation(
                        &plugin_id,
                        operation_id,
                        &output.artifact,
                        output.base_image_digest,
                        output.sdk_hash,
                    );
                    match write_candidate(
                        &operation_paths.candidate_dir,
                        &attestation,
                        &output.manifest,
                    ) {
                        Ok(()) => {
                            let _ = self
                                .set_built(
                                    &plugin_id,
                                    operation_id,
                                    output.artifact,
                                    output.build_logs,
                                    Utc::now(),
                                )
                                .await;
                        }
                        Err(error) => {
                            let _ = self
                                .inner
                                .adapter
                                .remove_image(&output.artifact.image_id)
                                .await;
                            let _ = self
                                .set_failed(
                                    &plugin_id,
                                    operation_id,
                                    error.error_code(),
                                    output.build_logs,
                                    Utc::now(),
                                )
                                .await;
                        }
                    }
                }
            }
            Err(error) => {
                let _ = self
                    .set_failed(
                        &plugin_id,
                        operation_id,
                        error.code(),
                        error.build_logs().to_vec(),
                        Utc::now(),
                    )
                    .await;
            }
        }
        let _ = self
            .inner
            .adapter
            .cleanup_temporary_resources(operation_id)
            .await;
    }

    async fn set_built(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        artifact: PluginInstallArtifact,
        build_logs: Vec<PluginBuildLog>,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let mut state = self.inner.state.lock().await;
        let stored = matching_operation_mut(&mut state, plugin_id, operation_id)?;
        if stored.operation.state != PluginInstallOperationState::Building {
            return Err(InstallOperationError::InvalidTransition);
        }
        stored.operation.state = PluginInstallOperationState::Built;
        stored.operation.artifact = Some(artifact);
        stored.operation.build_logs = build_logs;
        stored.operation.error_code = None;
        stored.updated_at = now;
        persist_record(&self.inner.root, stored)?;
        Ok(stored.operation.clone())
    }

    async fn set_failed(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        error_code: impl Into<String>,
        build_logs: Vec<PluginBuildLog>,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let mut state = self.inner.state.lock().await;
        let stored = matching_operation_mut(&mut state, plugin_id, operation_id)?;
        stored.operation.state = PluginInstallOperationState::Failed;
        stored.operation.error_code = Some(error_code.into());
        stored.operation.build_logs = build_logs;
        stored.updated_at = now;
        persist_record(&self.inner.root, stored)?;
        Ok(stored.operation.clone())
    }

    async fn set_state(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        state_value: PluginInstallOperationState,
        error_code: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, InstallOperationError> {
        let mut state = self.inner.state.lock().await;
        let stored = matching_operation_mut(&mut state, plugin_id, operation_id)?;
        if stored.operation.state.is_terminal() && stored.operation.state != state_value {
            return Err(InstallOperationError::InvalidTransition);
        }
        stored.operation.state = state_value;
        stored.operation.error_code = error_code;
        stored.updated_at = now;
        persist_record(&self.inner.root, stored)?;
        Ok(stored.operation.clone())
    }

    async fn reconcile_restart(&self, now: DateTime<Utc>) -> Result<(), InstallOperationError> {
        let operations = {
            let state = self.inner.state.lock().await;
            state
                .operations
                .values()
                .map(|stored| stored.operation.clone())
                .collect::<Vec<_>>()
        };

        for operation in operations {
            match operation.state {
                PluginInstallOperationState::Accepted | PluginInstallOperationState::Building => {
                    self.set_failed(
                        &operation.plugin_id,
                        operation.operation_id,
                        RESTART_ERROR_CODE,
                        Vec::new(),
                        now,
                    )
                    .await?;
                    let operation_paths = self.paths(&operation.plugin_id, operation.operation_id);
                    remove_directory(&operation_paths.candidate_dir)?;
                    if let Some(artifact) = operation.artifact {
                        self.inner.adapter.remove_image(&artifact.image_id).await?;
                    }
                    self.inner
                        .adapter
                        .cleanup_temporary_resources(operation.operation_id)
                        .await?;
                }
                PluginInstallOperationState::Finalized => {
                    self.finalize(&operation.plugin_id, operation.operation_id, now)
                        .await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn attestation(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        artifact: &PluginInstallArtifact,
        base_image_digest: String,
        sdk_hash: String,
    ) -> InstallAttestation {
        InstallAttestation {
            installation_id: self.inner.installation_id.clone(),
            operation_id,
            plugin_id: plugin_id.clone(),
            image_id: artifact.image_id.clone(),
            source_kind: "github_public_repository".to_string(),
            repository_id: artifact.repository_id.clone(),
            commit_sha: artifact.commit_sha.clone(),
            source_hash: artifact.source_hash.clone(),
            manifest_hash: artifact.manifest_hash.clone(),
            base_image_digest,
            sdk_hash,
            memory_bytes: 128 * 1024 * 1024,
            nano_cpus: 500_000_000,
            pids_limit: 64,
            run_mode: audiodown_domain::plugin::RunMode::OnDemand,
        }
    }

    fn attestation_from_directory(
        &self,
        directory: &Path,
        plugin_id: &PluginId,
        operation_id: Uuid,
        artifact: PluginInstallArtifact,
    ) -> Result<InstallAttestation, InstallOperationError> {
        let path = directory.join("install.json");
        let bytes =
            fs::read(&path).map_err(|_| InstallOperationError::InstalledAttestationMismatch)?;
        let attestation = serde_json::from_slice::<InstallAttestation>(&bytes)
            .map_err(|_| InstallOperationError::InstalledAttestationMismatch)?;
        if attestation.installation_id != self.inner.installation_id
            || attestation.operation_id != operation_id
            || attestation.plugin_id != *plugin_id
            || attestation.image_id != artifact.image_id
            || attestation.repository_id != artifact.repository_id
            || attestation.commit_sha != artifact.commit_sha
            || attestation.source_hash != artifact.source_hash
            || attestation.manifest_hash != artifact.manifest_hash
        {
            return Err(InstallOperationError::InstalledAttestationMismatch);
        }
        Ok(attestation)
    }
}

fn ensure_layout(root: &Path) -> Result<(), InstallOperationError> {
    for directory in [
        root.join("operations"),
        root.join("candidates"),
        root.join("installed"),
        root.join("prepared"),
        root.join("risk-grants"),
    ] {
        fs::create_dir_all(&directory).map_err(|error| io_error(&directory, error))?;
    }
    Ok(())
}

fn load_state(root: &Path) -> Result<OperationState, InstallOperationError> {
    let operations_dir = root.join("operations");
    let mut records = Vec::new();
    for entry in fs::read_dir(&operations_dir).map_err(|error| io_error(&operations_dir, error))? {
        let entry = entry.map_err(|error| io_error(&operations_dir, error))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
        let record = serde_json::from_slice::<StoredOperation>(&bytes)
            .map_err(|error| InstallOperationError::Json { path, error })?;
        records.push(record);
    }
    records.sort_by_key(|record| record.sequence);

    let mut state = OperationState::default();
    for record in records {
        state.next_sequence = state.next_sequence.max(record.sequence.saturating_add(1));
        state
            .operations
            .insert(record.operation.operation_id, record);
    }
    Ok(state)
}

fn matching_operation<'a>(
    state: &'a OperationState,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> Result<&'a StoredOperation, InstallOperationError> {
    let stored = state
        .operations
        .get(&operation_id)
        .ok_or(InstallOperationError::NotFound)?;
    if &stored.operation.plugin_id != plugin_id {
        return Err(InstallOperationError::NotFound);
    }
    Ok(stored)
}

fn matching_operation_mut<'a>(
    state: &'a mut OperationState,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> Result<&'a mut StoredOperation, InstallOperationError> {
    let stored = state
        .operations
        .get_mut(&operation_id)
        .ok_or(InstallOperationError::NotFound)?;
    if &stored.operation.plugin_id != plugin_id {
        return Err(InstallOperationError::NotFound);
    }
    Ok(stored)
}

fn paths(root: &Path, plugin_id: &PluginId, operation_id: Uuid) -> InstallOperationPaths {
    InstallOperationPaths {
        operation_record: operation_record_path(root, operation_id),
        candidate_dir: root.join("candidates").join(operation_id.to_string()),
        installed_dir: root.join("installed").join(plugin_id.as_str()),
        prepared_request: root.join("prepared").join(format!("{operation_id}.json")),
        mirrored_grant: root
            .join("risk-grants")
            .join(format!("{operation_id}.json")),
    }
}

fn operation_record_path(root: &Path, operation_id: Uuid) -> PathBuf {
    root.join("operations").join(format!("{operation_id}.json"))
}

fn persist_record(root: &Path, record: &StoredOperation) -> Result<(), InstallOperationError> {
    let path = operation_record_path(root, record.operation.operation_id);
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| InstallOperationError::Json {
        path: path.clone(),
        error,
    })?;
    atomic_write(&path, &bytes)
}

fn write_candidate(
    candidate_dir: &Path,
    attestation: &InstallAttestation,
    manifest: &[u8],
) -> Result<(), InstallOperationError> {
    if candidate_dir.exists() {
        return Err(InstallOperationError::CandidateAlreadyExists);
    }
    let parent = candidate_dir
        .parent()
        .ok_or(InstallOperationError::InvalidPath)?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temporary = parent.join(format!(".candidate-{}", Uuid::new_v4()));
    fs::create_dir(&temporary).map_err(|error| io_error(&temporary, error))?;

    let result = (|| {
        atomic_write(&temporary.join("audiodown-plugin.json"), manifest)?;
        let attestation_bytes = serde_json::to_vec_pretty(attestation).map_err(|error| {
            InstallOperationError::Json {
                path: temporary.join("install.json"),
                error,
            }
        })?;
        atomic_write(&temporary.join("install.json"), &attestation_bytes)?;
        sync_directory(&temporary)?;
        fs::rename(&temporary, candidate_dir).map_err(|error| io_error(&temporary, error))?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

fn verify_attestation(
    directory: &Path,
    expected: &InstallAttestation,
) -> Result<(), InstallOperationError> {
    let path = directory.join("install.json");
    let bytes = fs::read(&path).map_err(|_| InstallOperationError::InstalledAttestationMismatch)?;
    let actual = serde_json::from_slice::<InstallAttestation>(&bytes)
        .map_err(|_| InstallOperationError::InstalledAttestationMismatch)?;
    if &actual != expected {
        return Err(InstallOperationError::InstalledAttestationMismatch);
    }
    Ok(())
}

fn remove_mirrors(root: &Path, paths: &InstallOperationPaths) -> Result<(), InstallOperationError> {
    if let Ok(bytes) = fs::read(&paths.prepared_request) {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(grant_id) = value
                .get("riskGrantId")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
            {
                remove_file_if_exists(&root.join("grants").join(format!("{grant_id}.json")))?;
            }
        }
    }
    remove_file_if_exists(&paths.prepared_request)?;
    remove_file_if_exists(&paths.mirrored_grant)
}

fn remove_file_if_exists(path: &Path) -> Result<(), InstallOperationError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn remove_directory(path: &Path) -> Result<(), InstallOperationError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), InstallOperationError> {
    let parent = path.parent().ok_or(InstallOperationError::InvalidPath)?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temporary = parent.join(format!(".write-{}", Uuid::new_v4()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| io_error(&temporary, error))?;
        file.write_all(bytes)
            .map_err(|error| io_error(&temporary, error))?;
        file.sync_all()
            .map_err(|error| io_error(&temporary, error))?;
        drop(file);
        fs::rename(&temporary, path).map_err(|error| io_error(&temporary, error))?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sync_directory(path: &Path) -> Result<(), InstallOperationError> {
    let directory = fs::File::open(path).map_err(|error| io_error(path, error))?;
    directory.sync_all().map_err(|error| io_error(path, error))
}

fn io_error(path: &Path, source: std::io::Error) -> InstallOperationError {
    InstallOperationError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallOperationError {
    #[error("operation ID belongs to another plugin")]
    OperationIdMismatch,
    #[error("another plugin build is active")]
    BuildBusy,
    #[error("unacknowledged operation capacity has been reached")]
    OperationCapacityReached,
    #[error("operation was not found")]
    NotFound,
    #[error("operation cannot transition from its current state")]
    InvalidTransition,
    #[error("operation is not terminal")]
    NotTerminal,
    #[error("installed plugin attestation does not match the operation")]
    InstalledAttestationMismatch,
    #[error("candidate directory already exists")]
    CandidateAlreadyExists,
    #[error("operation path is invalid")]
    InvalidPath,
    #[error("build adapter failed: {0}")]
    Adapter(#[from] BuildAdapterError),
    #[error("failed to access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid operation data at {path}")]
    Json {
        path: PathBuf,
        #[source]
        error: serde_json::Error,
    },
}

impl InstallOperationError {
    fn error_code(&self) -> &str {
        match self {
            Self::CandidateAlreadyExists => "CANDIDATE_ALREADY_EXISTS",
            Self::InstalledAttestationMismatch => "INSTALL_ATTESTATION_MISMATCH",
            _ => "BUILD_STATE_FAILED",
        }
    }
}
