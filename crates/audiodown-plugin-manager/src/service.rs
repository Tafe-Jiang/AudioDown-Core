use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Duration,
};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus};
use audiodown_plugin_api::manifest::PluginType;
use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact, PluginInstallOperation,
    PluginInstallOperationList, PluginInstallOperationState, PluginRemoveResult,
    PluginRuntimeState,
};
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use semver::Version;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, Semaphore};
use uuid::Uuid;

use crate::{
    archive::{extract_snapshot, SnapshotLimits},
    github::GitHubRepositoryRef,
    staging::{LifecycleRiskGrant, PluginPreview, SnapshotStore, StagedPlugin},
    validation::validate_repository,
    PluginManagerError, RepositorySource,
};

const MAX_CONCURRENT_INSPECTIONS: usize = 2;
const LIFECYCLE_RISK_KIND: &str = "npm_lifecycle_scripts";
const DEFAULT_INSTALL_POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_INSTALL_WAIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[async_trait]
pub trait PluginStateStore: Send + Sync {
    async fn is_installed(&self, plugin_id: &PluginId) -> Result<bool, PluginManagerError>;

    async fn persist_risk_grant(
        &self,
        _grant: &LifecycleRiskGrant,
    ) -> Result<(), PluginManagerError> {
        Err(PluginManagerError::PluginStateUnavailable)
    }

    async fn insert_installing(
        &self,
        _record: &InstallPluginRecord,
    ) -> Result<(), PluginManagerError> {
        Err(PluginManagerError::PluginStateUnavailable)
    }

    async fn complete_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<InstallPluginRecord, PluginManagerError> {
        Err(PluginManagerError::PluginStateUnavailable)
    }

    async fn rollback_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<(), PluginManagerError> {
        Err(PluginManagerError::PluginStateUnavailable)
    }

    async fn list_install_records(&self) -> Result<Vec<InstallPluginRecord>, PluginManagerError> {
        Err(PluginManagerError::PluginStateUnavailable)
    }

    async fn persist_build_log(
        &self,
        _record: &PluginBuildLogRecord,
    ) -> Result<(), PluginManagerError> {
        Err(PluginManagerError::PluginStateUnavailable)
    }
}

#[async_trait]
pub trait PluginRuntimeControl: Send + Sync {
    async fn start(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError>;
    async fn stop(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError>;
    async fn inspect(&self, plugin_id: &PluginId)
        -> Result<PluginRuntimeState, PluginManagerError>;
    async fn remove(&self, plugin_id: &PluginId) -> Result<PluginRemoveResult, PluginManagerError>;
    async fn begin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError>;
    async fn install_status(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError>;
    async fn finalize_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError>;
    async fn abort_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError>;
    async fn list_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, PluginManagerError>;
    async fn acknowledge_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError>;
}

#[async_trait]
pub trait LifecycleRiskAuthorizer: Send + Sync {
    async fn authorize(
        &self,
        token: Option<&SecretString>,
    ) -> Result<(), LifecycleAuthorizationError>;
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAuthorizationError {
    #[error("developer mode is required")]
    DeveloperModeRequired,
    #[error("a valid development token is required")]
    TokenRequired,
}

pub struct PluginManagerService {
    state_store: Arc<dyn PluginStateStore>,
    repository_source: Arc<dyn RepositorySource>,
    runtime: Arc<dyn PluginRuntimeControl>,
    risk_authorizer: Arc<dyn LifecycleRiskAuthorizer>,
    snapshots: SnapshotStore,
    core_version: Version,
    plugin_api_version: Version,
    inspection_permits: Semaphore,
    operation_locks: StdMutex<HashMap<PluginId, Weak<AsyncMutex<()>>>>,
    install_poll_interval: Duration,
    install_wait_timeout: Duration,
}

impl PluginManagerService {
    pub fn new(
        state_store: Arc<dyn PluginStateStore>,
        repository_source: Arc<dyn RepositorySource>,
        plugin_data: PathBuf,
        core_version: Version,
        plugin_api_version: Version,
    ) -> Self {
        Self {
            state_store,
            repository_source,
            runtime: Arc::new(UnavailablePluginRuntime),
            risk_authorizer: Arc::new(DenyLifecycleRisk),
            snapshots: SnapshotStore::new(plugin_data),
            core_version,
            plugin_api_version,
            inspection_permits: Semaphore::new(MAX_CONCURRENT_INSPECTIONS),
            operation_locks: StdMutex::new(HashMap::new()),
            install_poll_interval: DEFAULT_INSTALL_POLL_INTERVAL,
            install_wait_timeout: DEFAULT_INSTALL_WAIT_TIMEOUT,
        }
    }

    pub fn with_installation_ports(
        mut self,
        runtime: Arc<dyn PluginRuntimeControl>,
        risk_authorizer: Arc<dyn LifecycleRiskAuthorizer>,
    ) -> Self {
        self.runtime = runtime;
        self.risk_authorizer = risk_authorizer;
        self
    }

    pub fn with_install_timing(mut self, poll_interval: Duration, wait_timeout: Duration) -> Self {
        self.install_poll_interval = poll_interval;
        self.install_wait_timeout = wait_timeout;
        self
    }

    pub async fn inspect_repository(
        &self,
        repository_url: &str,
    ) -> Result<RepositoryInspection, InspectionError> {
        self.inspect_repository_at(repository_url, Utc::now()).await
    }

    pub async fn inspect_repository_at(
        &self,
        repository_url: &str,
        now: DateTime<Utc>,
    ) -> Result<RepositoryInspection, InspectionError> {
        let source = GitHubRepositoryRef::parse(repository_url)
            .map_err(|_| InspectionError::InvalidRepositoryUrl)?;
        let _permit = self
            .inspection_permits
            .try_acquire()
            .map_err(|_| InspectionError::Busy)?;

        self.snapshots
            .cleanup_expired(now)
            .await
            .map_err(|_| InspectionError::Internal)?;

        let incoming_root = self.snapshots.plugin_data().join("incoming");
        std::fs::create_dir_all(&incoming_root).map_err(|_| InspectionError::Internal)?;
        let operation_root = incoming_root.join(Uuid::new_v4().to_string());
        std::fs::create_dir(&operation_root).map_err(|_| InspectionError::Internal)?;
        let cleanup = IncomingCleanup(operation_root.clone());

        let downloaded = self
            .repository_source
            .resolve_and_download(&source, &operation_root)
            .await
            .map_err(map_download_error)?;
        let extracted = extract_snapshot(
            &downloaded.archive_path,
            &operation_root.join("extracted"),
            SnapshotLimits::default(),
        )
        .await
        .map_err(map_repository_error)?;
        let validated = validate_repository(
            &extracted.repository_root,
            &self.core_version,
            &self.plugin_api_version,
            SnapshotLimits::default(),
        )
        .map_err(map_repository_error)?;
        let preview = self
            .snapshots
            .create(&source, &downloaded.commit_sha, extracted, validated)
            .await
            .map_err(|_| InspectionError::Internal)?;

        let mut plugins = Vec::with_capacity(preview.plugins.len());
        for plugin in preview.plugins {
            let already_installed = self
                .state_store
                .is_installed(&plugin.plugin_id)
                .await
                .map_err(|_| InspectionError::Internal)?;
            plugins.push(InspectedPlugin::from_preview(plugin, already_installed));
        }

        drop(cleanup);
        Ok(RepositoryInspection {
            snapshot_id: preview.snapshot_id,
            repository: InspectedRepository {
                id: preview.repository_id,
                name: preview.repository_name,
                source_url: preview.source_url,
                commit_sha: preview.commit_sha,
            },
            plugins,
        })
    }

    pub async fn install(
        &self,
        command: InstallPluginCommand,
    ) -> Result<InstallPluginRecord, InstallError> {
        let _operation_guard = self.try_operation_lock(&command.plugin_id)?;
        let staged = self
            .snapshots
            .load_plugin(command.snapshot_id, &command.plugin_id, Utc::now())
            .map_err(map_staged_error)?;
        if let Some(existing) = self
            .state_store
            .list_install_records()
            .await
            .map_err(|_| InstallError::Internal)?
            .into_iter()
            .find(|record| record.plugin_id == command.plugin_id)
        {
            return if existing.status == PluginStatus::Installing {
                Err(InstallError::PluginOperationInProgress)
            } else {
                Err(InstallError::PluginAlreadyInstalled)
            };
        }

        let grant = self
            .authorize_lifecycle_risk(&staged, command.lifecycle_risk)
            .await?;
        if let Some(grant) = &grant {
            self.state_store
                .persist_risk_grant(grant)
                .await
                .map_err(|_| InstallError::Internal)?;
        }

        let prepared = self
            .snapshots
            .prepare_install(command.snapshot_id, &command.plugin_id, grant.as_ref())
            .await
            .map_err(map_staged_error)?;

        let started_at = tokio::time::Instant::now();
        let initial = match self
            .runtime
            .begin_install(&command.plugin_id, prepared.operation_id)
            .await
        {
            Ok(operation) => operation,
            Err(_) => {
                self.recover_ambiguous_begin(&command.plugin_id, prepared.operation_id, started_at)
                    .await?
            }
        };
        let (built, build_logs) = self
            .wait_until_built(&staged, prepared.operation_id, initial, started_at)
            .await?;
        let artifact = built
            .artifact
            .as_ref()
            .filter(|artifact| artifact_matches_staged(artifact, &staged))
            .cloned();
        let Some(artifact) = artifact else {
            self.abort_and_ack(&command.plugin_id, prepared.operation_id)
                .await;
            self.persist_build_logs(&staged, prepared.operation_id, &build_logs)
                .await?;
            return Err(InstallError::ArtifactMismatch);
        };

        let installing = install_record(&staged, prepared.operation_id, &artifact);
        if self
            .state_store
            .insert_installing(&installing)
            .await
            .is_err()
        {
            self.abort_and_ack(&command.plugin_id, prepared.operation_id)
                .await;
            self.persist_build_logs(&staged, prepared.operation_id, &build_logs)
                .await?;
            return Err(InstallError::Internal);
        }
        self.persist_build_logs(&staged, prepared.operation_id, &build_logs)
            .await?;

        let finalized = self
            .runtime
            .finalize_install(&command.plugin_id, prepared.operation_id)
            .await
            .map_err(|_| InstallError::RuntimeUnavailable)?;
        if finalized.state != PluginInstallOperationState::Finalized
            || !operation_identity_matches(&finalized, &command.plugin_id, prepared.operation_id)
            || finalized
                .artifact
                .as_ref()
                .is_none_or(|value| !artifact_matches_staged(value, &staged))
        {
            return Err(InstallError::RuntimeUnavailable);
        }

        let completed = self
            .state_store
            .complete_install(&command.plugin_id, prepared.operation_id)
            .await
            .map_err(|_| InstallError::Internal)?;
        self.acknowledge(&command.plugin_id, prepared.operation_id)
            .await?;
        Ok(completed)
    }

    pub async fn reconcile_install_operations(&self) -> Result<(), InstallError> {
        let operations = self
            .runtime
            .list_install_operations()
            .await
            .map_err(|_| InstallError::RuntimeUnavailable)?;
        let records = self
            .state_store
            .list_install_records()
            .await
            .map_err(|_| InstallError::Internal)?;

        let mut listed = HashSet::new();
        let mut first_error = None;
        for operation in operations.operations {
            listed.insert((operation.plugin_id.clone(), operation.operation_id));
            let record = records
                .iter()
                .find(|record| record.plugin_id == operation.plugin_id);
            let result = async {
                let detailed = self
                    .runtime
                    .install_status(&operation.plugin_id, operation.operation_id)
                    .await
                    .map_err(|_| InstallError::RuntimeUnavailable)?;
                if !operation_identity_matches(
                    &detailed,
                    &operation.plugin_id,
                    operation.operation_id,
                ) {
                    return Err(InstallError::RuntimeUnavailable);
                }
                self.persist_reconciled_build_logs(record, &detailed)
                    .await?;
                self.reconcile_operation(detailed.summary(), record).await
            }
            .await;
            if first_error.is_none() {
                first_error = result.err();
            }
        }

        for record in records.iter().filter(|record| {
            record.status == PluginStatus::Installing
                && record.install_operation_id.is_some_and(|operation_id| {
                    !listed.contains(&(record.plugin_id.clone(), operation_id))
                })
        }) {
            let operation_id = record
                .install_operation_id
                .expect("filtered installing record must have an operation ID");
            let result = async {
                let detailed = self
                    .runtime
                    .install_status(&record.plugin_id, operation_id)
                    .await
                    .map_err(|_| InstallError::RuntimeUnavailable)?;
                if !operation_identity_matches(&detailed, &record.plugin_id, operation_id) {
                    return Err(InstallError::RuntimeUnavailable);
                }
                self.persist_reconciled_build_logs(Some(record), &detailed)
                    .await?;
                self.reconcile_operation(detailed.summary(), Some(record))
                    .await
            }
            .await;
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn reconcile_operation(
        &self,
        operation: audiodown_supervisor_protocol::PluginInstallOperationSummary,
        record: Option<&InstallPluginRecord>,
    ) -> Result<(), InstallError> {
        let matching_installing = record.filter(|record| {
            record.status == PluginStatus::Installing
                && record.install_operation_id == Some(operation.operation_id)
        });
        let matching_installed = record.filter(|record| {
            record.status == PluginStatus::Installed
                && record.install_operation_id.is_none()
                && operation
                    .artifact
                    .as_ref()
                    .is_some_and(|artifact| artifact_matches_record(artifact, record))
        });

        match operation.state {
            PluginInstallOperationState::Accepted | PluginInstallOperationState::Building => {
                if matching_installing.is_none() {
                    self.abort_and_ack_result(&operation.plugin_id, operation.operation_id)
                        .await?;
                }
            }
            PluginInstallOperationState::Built => {
                if let Some(record) = matching_installing {
                    if operation
                        .artifact
                        .as_ref()
                        .is_none_or(|artifact| !artifact_matches_record(artifact, record))
                    {
                        return Err(InstallError::ArtifactMismatch);
                    }
                    let finalized = self
                        .runtime
                        .finalize_install(&operation.plugin_id, operation.operation_id)
                        .await
                        .map_err(|_| InstallError::RuntimeUnavailable)?;
                    if finalized.state != PluginInstallOperationState::Finalized
                        || !operation_identity_matches(
                            &finalized,
                            &operation.plugin_id,
                            operation.operation_id,
                        )
                        || finalized
                            .artifact
                            .as_ref()
                            .is_none_or(|artifact| !artifact_matches_record(artifact, record))
                    {
                        return Err(InstallError::RuntimeUnavailable);
                    }
                    self.state_store
                        .complete_install(&operation.plugin_id, operation.operation_id)
                        .await
                        .map_err(|_| InstallError::Internal)?;
                    self.acknowledge(&operation.plugin_id, operation.operation_id)
                        .await?;
                } else {
                    self.abort_and_ack_result(&operation.plugin_id, operation.operation_id)
                        .await?;
                }
            }
            PluginInstallOperationState::Finalized => {
                if let Some(record) = matching_installing {
                    if operation
                        .artifact
                        .as_ref()
                        .is_none_or(|artifact| !artifact_matches_record(artifact, record))
                    {
                        return Err(InstallError::ArtifactMismatch);
                    }
                    self.state_store
                        .complete_install(&operation.plugin_id, operation.operation_id)
                        .await
                        .map_err(|_| InstallError::Internal)?;
                    self.acknowledge(&operation.plugin_id, operation.operation_id)
                        .await?;
                } else if matching_installed.is_some() {
                    self.acknowledge(&operation.plugin_id, operation.operation_id)
                        .await?;
                } else {
                    self.abort_and_ack_result(&operation.plugin_id, operation.operation_id)
                        .await?;
                }
            }
            PluginInstallOperationState::Failed | PluginInstallOperationState::Aborted => {
                let aborted = self
                    .runtime
                    .abort_install(&operation.plugin_id, operation.operation_id)
                    .await
                    .map_err(|_| InstallError::RuntimeUnavailable)?;
                if aborted.state != PluginInstallOperationState::Aborted
                    || !operation_identity_matches(
                        &aborted,
                        &operation.plugin_id,
                        operation.operation_id,
                    )
                {
                    return Err(InstallError::RuntimeUnavailable);
                }
                if matching_installing.is_some() {
                    self.state_store
                        .rollback_install(&operation.plugin_id, operation.operation_id)
                        .await
                        .map_err(|_| InstallError::Internal)?;
                }
                self.acknowledge(&operation.plugin_id, operation.operation_id)
                    .await?;
            }
        }
        Ok(())
    }

    async fn authorize_lifecycle_risk(
        &self,
        staged: &StagedPlugin,
        input: LifecycleRiskInput,
    ) -> Result<Option<LifecycleRiskGrant>, InstallError> {
        if !staged.requires_lifecycle_scripts {
            return Ok(None);
        }
        if !input.explicitly_approved {
            return Err(InstallError::RiskGrantRequired);
        }
        self.risk_authorizer
            .authorize(input.developer_token.as_ref())
            .await
            .map_err(|error| match error {
                LifecycleAuthorizationError::DeveloperModeRequired => {
                    InstallError::DeveloperModeRequired
                }
                LifecycleAuthorizationError::TokenRequired => InstallError::DevTokenRequired,
            })?;
        let reason = staged
            .lifecycle_script_reason
            .clone()
            .ok_or(InstallError::RiskGrantRequired)?;
        Ok(Some(LifecycleRiskGrant {
            id: Uuid::new_v4(),
            repository_id: staged.repository_id.clone(),
            plugin_id: staged.manifest.id.clone(),
            commit_sha: staged.commit_sha.clone(),
            risk_kind: LIFECYCLE_RISK_KIND.to_string(),
            reason,
            granted_at: Utc::now(),
        }))
    }

    async fn wait_until_built(
        &self,
        staged: &StagedPlugin,
        operation_id: Uuid,
        mut operation: PluginInstallOperation,
        started_at: tokio::time::Instant,
    ) -> Result<(PluginInstallOperation, Vec<PluginBuildLog>), InstallError> {
        let mut logs = Vec::new();
        loop {
            if !operation_identity_matches(&operation, &staged.manifest.id, operation_id) {
                return Err(InstallError::RuntimeUnavailable);
            }
            merge_build_logs(&mut logs, &operation.build_logs);
            match operation.state {
                PluginInstallOperationState::Built => return Ok((operation, logs)),
                PluginInstallOperationState::Failed | PluginInstallOperationState::Aborted => {
                    let log_result = self.persist_build_logs(staged, operation_id, &logs).await;
                    self.abort_and_ack_result(&staged.manifest.id, operation_id)
                        .await?;
                    log_result?;
                    return Err(InstallError::BuildFailed);
                }
                PluginInstallOperationState::Finalized => {
                    return Err(InstallError::RuntimeUnavailable);
                }
                PluginInstallOperationState::Accepted | PluginInstallOperationState::Building => {}
            }
            if started_at.elapsed() >= self.install_wait_timeout {
                let log_result = self.persist_build_logs(staged, operation_id, &logs).await;
                self.abort_and_ack_result(&staged.manifest.id, operation_id)
                    .await?;
                log_result?;
                return Err(InstallError::InstallTimeout);
            }
            tokio::time::sleep(self.install_poll_interval).await;
            operation = match self
                .runtime
                .install_status(&staged.manifest.id, operation_id)
                .await
            {
                Ok(operation) => operation,
                Err(_) => {
                    self.persist_build_logs(staged, operation_id, &logs).await?;
                    return Err(InstallError::RuntimeUnavailable);
                }
            };
        }
    }

    async fn recover_ambiguous_begin(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        started_at: tokio::time::Instant,
    ) -> Result<PluginInstallOperation, InstallError> {
        loop {
            match self.runtime.install_status(plugin_id, operation_id).await {
                Ok(operation) => return Ok(operation),
                Err(_) if started_at.elapsed() < self.install_wait_timeout => {
                    tokio::time::sleep(self.install_poll_interval).await;
                }
                Err(_) => {
                    self.abort_and_ack(plugin_id, operation_id).await;
                    return Err(InstallError::InstallTimeout);
                }
            }
        }
    }

    async fn persist_build_logs(
        &self,
        staged: &StagedPlugin,
        operation_id: Uuid,
        logs: &[PluginBuildLog],
    ) -> Result<(), InstallError> {
        for log in logs {
            self.state_store
                .persist_build_log(&PluginBuildLogRecord {
                    operation_id,
                    plugin_id: staged.manifest.id.clone(),
                    plugin_version: staged.manifest.version.to_string(),
                    platform_id: staged.manifest.platform.id.clone(),
                    sequence: log.sequence,
                    stream: log.stream,
                    message: redact_build_log(&log.message),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(|_| InstallError::Internal)?;
        }
        Ok(())
    }

    async fn persist_reconciled_build_logs(
        &self,
        record: Option<&InstallPluginRecord>,
        operation: &PluginInstallOperation,
    ) -> Result<(), InstallError> {
        for log in &operation.build_logs {
            self.state_store
                .persist_build_log(&PluginBuildLogRecord {
                    operation_id: operation.operation_id,
                    plugin_id: operation.plugin_id.clone(),
                    plugin_version: record
                        .map(|value| value.version.clone())
                        .unwrap_or_default(),
                    platform_id: record
                        .map(|value| value.platform_id.clone())
                        .unwrap_or_default(),
                    sequence: log.sequence,
                    stream: log.stream,
                    message: redact_build_log(&log.message),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(|_| InstallError::Internal)?;
        }
        Ok(())
    }

    async fn abort_and_ack(&self, plugin_id: &PluginId, operation_id: Uuid) {
        let _ = self.abort_and_ack_result(plugin_id, operation_id).await;
    }

    async fn abort_and_ack_result(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<(), InstallError> {
        let aborted = self
            .runtime
            .abort_install(plugin_id, operation_id)
            .await
            .map_err(|_| InstallError::RuntimeUnavailable)?;
        if aborted.state != PluginInstallOperationState::Aborted
            || !operation_identity_matches(&aborted, plugin_id, operation_id)
        {
            return Err(InstallError::RuntimeUnavailable);
        }
        self.acknowledge(plugin_id, operation_id).await
    }

    async fn acknowledge(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<(), InstallError> {
        let acknowledged = self
            .runtime
            .acknowledge_install(plugin_id, operation_id)
            .await
            .map_err(|_| InstallError::RuntimeUnavailable)?;
        if !acknowledged.acknowledged
            || !operation_identity_matches(&acknowledged, plugin_id, operation_id)
        {
            return Err(InstallError::RuntimeUnavailable);
        }
        Ok(())
    }

    fn try_operation_lock(
        &self,
        plugin_id: &PluginId,
    ) -> Result<OwnedMutexGuard<()>, InstallError> {
        let lock = {
            let mut registry = self
                .operation_locks
                .lock()
                .map_err(|_| InstallError::Internal)?;
            registry.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = registry.get(plugin_id).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                registry.insert(plugin_id.clone(), Arc::downgrade(&lock));
                lock
            }
        };
        lock.try_lock_owned()
            .map_err(|_| InstallError::PluginOperationInProgress)
    }
}

#[derive(Debug)]
pub struct InstallPluginCommand {
    pub snapshot_id: Uuid,
    pub plugin_id: PluginId,
    pub lifecycle_risk: LifecycleRiskInput,
}

#[derive(Debug)]
pub struct LifecycleRiskInput {
    pub explicitly_approved: bool,
    pub developer_token: Option<SecretString>,
}

#[derive(Debug, Clone)]
pub struct InstallPluginRecord {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
    pub plugin_type: PluginType,
    pub platform_id: String,
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    pub source_ref: String,
    pub commit_sha: String,
    pub repository_id: String,
    pub manifest_json: serde_json::Value,
    pub manifest_hash: String,
    pub source_hash: String,
    pub image_id: Option<String>,
    pub status: PluginStatus,
    pub install_operation_id: Option<Uuid>,
    pub installed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PluginBuildLogRecord {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
    pub plugin_version: String,
    pub platform_id: String,
    pub sequence: u64,
    pub stream: PluginBuildLogStream,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum InstallError {
    #[error("staged snapshot was not found")]
    SnapshotNotFound,
    #[error("plugin is not present in the staged snapshot")]
    PluginNotInSnapshot,
    #[error("plugin is already installed")]
    PluginAlreadyInstalled,
    #[error("another operation is already running for this plugin")]
    PluginOperationInProgress,
    #[error("lifecycle-script risk approval is required")]
    RiskGrantRequired,
    #[error("developer mode is required")]
    DeveloperModeRequired,
    #[error("a valid development token is required")]
    DevTokenRequired,
    #[error("plugin build failed")]
    BuildFailed,
    #[error("plugin build timed out")]
    InstallTimeout,
    #[error("plugin runtime service is unavailable")]
    RuntimeUnavailable,
    #[error("plugin build artifact does not match the staged plugin")]
    ArtifactMismatch,
    #[error("plugin installation failed")]
    Internal,
}

struct IncomingCleanup(PathBuf);

impl Drop for IncomingCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum InspectionError {
    #[error("invalid repository URL")]
    InvalidRepositoryUrl,
    #[error("repository service is unavailable")]
    RepositoryUnavailable,
    #[error("repository content is invalid")]
    InvalidRepository,
    #[error("repository inspection is busy")]
    Busy,
    #[error("repository inspection failed")]
    Internal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInspection {
    pub snapshot_id: Uuid,
    pub repository: InspectedRepository,
    pub plugins: Vec<InspectedPlugin>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedRepository {
    pub id: String,
    pub name: String,
    pub source_url: String,
    pub commit_sha: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedPlugin {
    pub plugin_id: PluginId,
    pub name: String,
    pub version: Version,
    pub plugin_type: PluginType,
    pub already_installed: bool,
    pub requires_lifecycle_script_grant: bool,
    pub lifecycle_script_reason: Option<String>,
}

impl InspectedPlugin {
    fn from_preview(preview: PluginPreview, already_installed: bool) -> Self {
        Self {
            plugin_id: preview.plugin_id,
            name: preview.name,
            version: preview.version,
            plugin_type: preview.plugin_type,
            already_installed,
            requires_lifecycle_script_grant: preview.requires_lifecycle_script_grant,
            lifecycle_script_reason: preview.lifecycle_script_reason,
        }
    }
}

struct UnavailablePluginRuntime;

#[async_trait]
impl PluginRuntimeControl for UnavailablePluginRuntime {
    async fn start(&self, _plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn stop(&self, _plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn inspect(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn remove(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRemoveResult, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
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
        Err(PluginManagerError::RuntimeUnavailable)
    }

    async fn acknowledge_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        Err(PluginManagerError::RuntimeUnavailable)
    }
}

struct DenyLifecycleRisk;

#[async_trait]
impl LifecycleRiskAuthorizer for DenyLifecycleRisk {
    async fn authorize(
        &self,
        _token: Option<&SecretString>,
    ) -> Result<(), LifecycleAuthorizationError> {
        Err(LifecycleAuthorizationError::DeveloperModeRequired)
    }
}

fn map_download_error(error: PluginManagerError) -> InspectionError {
    match error {
        PluginManagerError::InvalidRepositoryUrl => InspectionError::InvalidRepositoryUrl,
        _ => InspectionError::RepositoryUnavailable,
    }
}

fn map_repository_error(_error: PluginManagerError) -> InspectionError {
    InspectionError::InvalidRepository
}

fn map_staged_error(error: PluginManagerError) -> InstallError {
    match error {
        PluginManagerError::SnapshotNotFound => InstallError::SnapshotNotFound,
        PluginManagerError::InvalidStagingMetadata => InstallError::PluginNotInSnapshot,
        _ => InstallError::Internal,
    }
}

fn install_record(
    staged: &StagedPlugin,
    operation_id: Uuid,
    artifact: &PluginInstallArtifact,
) -> InstallPluginRecord {
    InstallPluginRecord {
        operation_id,
        plugin_id: staged.manifest.id.clone(),
        plugin_type: staged.manifest.plugin_type,
        platform_id: staged.manifest.platform.id.clone(),
        name: staged.manifest.name.clone(),
        version: staged.manifest.version.to_string(),
        protocol_version: staged.manifest.schema_version.clone(),
        source_ref: staged.source_url.clone(),
        commit_sha: staged.commit_sha.clone(),
        repository_id: staged.repository_id.clone(),
        manifest_json: serde_json::to_value(&staged.manifest).unwrap_or(serde_json::Value::Null),
        manifest_hash: staged.manifest_hash.clone(),
        source_hash: staged.source_hash.clone(),
        image_id: Some(artifact.image_id.clone()),
        status: PluginStatus::Installing,
        install_operation_id: Some(operation_id),
        installed_at: Utc::now(),
    }
}

fn artifact_matches_staged(artifact: &PluginInstallArtifact, staged: &StagedPlugin) -> bool {
    artifact.repository_id == staged.repository_id
        && artifact.commit_sha == staged.commit_sha
        && artifact.source_hash == staged.source_hash
        && artifact.manifest_hash == staged.manifest_hash
}

fn operation_identity_matches(
    operation: &PluginInstallOperation,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> bool {
    &operation.plugin_id == plugin_id && operation.operation_id == operation_id
}

fn artifact_matches_record(artifact: &PluginInstallArtifact, record: &InstallPluginRecord) -> bool {
    artifact.repository_id == record.repository_id
        && artifact.commit_sha == record.commit_sha
        && artifact.source_hash == record.source_hash
        && artifact.manifest_hash == record.manifest_hash
        && record.image_id.as_deref() == Some(artifact.image_id.as_str())
}

fn merge_build_logs(target: &mut Vec<PluginBuildLog>, incoming: &[PluginBuildLog]) {
    for log in incoming {
        if !target
            .iter()
            .any(|existing| existing.sequence == log.sequence)
        {
            target.push(log.clone());
        }
    }
    target.sort_by_key(|log| log.sequence);
}

fn redact_build_log(message: &str) -> String {
    let redacted = audiodown_logging::redact_text(message);
    let assignment = regex::Regex::new(r"(?i)\b(token|access_token|password|secret)=([^\s;&]+)")
        .expect("build-log redaction regex must compile");
    assignment
        .replace_all(&redacted, "$1=[REDACTED]")
        .into_owned()
}
