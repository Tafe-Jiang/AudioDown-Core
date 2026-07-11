use async_trait::async_trait;
use audiodown_domain::{
    log::{LogLevel, StructuredLog},
    plugin::PluginId,
};
use audiodown_plugin_manager::{
    service::{
        InstallPluginRecord, LifecycleAuthorizationError, LifecycleRiskAuthorizer,
        PluginBuildLogRecord, PluginRuntimeControl, PluginRuntimeLogRecord, PluginStateStore,
    },
    staging::LifecycleRiskGrant,
    DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use audiodown_storage::{PluginRecord, RiskGrantRecord, Storage};
use audiodown_supervisor_protocol::{
    PluginInstallOperation, PluginInstallOperationList, PluginRemoveResult, PluginRuntimeState,
};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    state::DevelopmentConfig,
    supervisor::{SupervisorClient, SupervisorError},
};

#[derive(Clone)]
pub struct SqlitePluginManagerStore {
    storage: Storage,
}

impl SqlitePluginManagerStore {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl PluginStateStore for SqlitePluginManagerStore {
    async fn is_installed(&self, plugin_id: &PluginId) -> Result<bool, PluginManagerError> {
        self.storage
            .plugins()
            .get(plugin_id)
            .await
            .map(|record| record.is_some())
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn persist_risk_grant(
        &self,
        grant: &LifecycleRiskGrant,
    ) -> Result<(), PluginManagerError> {
        self.storage
            .risk_grants()
            .replace_commit_grant(&RiskGrantRecord {
                id: grant.id,
                repository_id: grant.repository_id.clone(),
                plugin_id: grant.plugin_id.clone(),
                commit_sha: grant.commit_sha.clone(),
                risk_kind: grant.risk_kind.clone(),
                reason: grant.reason.clone(),
                granted_at: grant.granted_at,
            })
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn insert_installing(
        &self,
        record: &InstallPluginRecord,
    ) -> Result<(), PluginManagerError> {
        self.storage
            .plugins()
            .insert_installing(&storage_record(record))
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn complete_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<InstallPluginRecord, PluginManagerError> {
        self.storage
            .plugins()
            .complete_install(plugin_id, operation_id)
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)?;
        let record = self
            .storage
            .plugins()
            .get(plugin_id)
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)?
            .ok_or(PluginManagerError::PluginStateUnavailable)?;
        Ok(manager_record(record, operation_id))
    }

    async fn rollback_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<(), PluginManagerError> {
        self.storage
            .plugins()
            .rollback_install(plugin_id, operation_id)
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn list_install_records(&self) -> Result<Vec<InstallPluginRecord>, PluginManagerError> {
        self.storage
            .plugins()
            .list()
            .await
            .map(|records| {
                records
                    .into_iter()
                    .map(|record| {
                        let operation_id = record.install_operation_id.unwrap_or(Uuid::nil());
                        manager_record(record, operation_id)
                    })
                    .collect()
            })
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn persist_build_log(
        &self,
        record: &PluginBuildLogRecord,
    ) -> Result<(), PluginManagerError> {
        self.storage
            .logs()
            .append_if_absent(&StructuredLog {
                id: Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("{}:{}", record.operation_id, record.sequence).as_bytes(),
                ),
                timestamp: record.timestamp,
                level: match record.stream {
                    audiodown_supervisor_protocol::PluginBuildLogStream::Stderr => LogLevel::Warn,
                    _ => LogLevel::Info,
                },
                component: "plugin-build".to_string(),
                message: record.message.clone(),
                plugin_id: Some(record.plugin_id.to_string()),
                plugin_version: non_empty(&record.plugin_version),
                platform_id: non_empty(&record.platform_id),
                request_id: Some(record.operation_id.to_string()),
                task_id: None,
                container_id: None,
                error_code: None,
                context: serde_json::json!({
                    "operationId": record.operation_id,
                    "sequence": record.sequence,
                    "stream": record.stream,
                }),
            })
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn get_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<InstallPluginRecord>, PluginManagerError> {
        self.storage
            .plugins()
            .get(plugin_id)
            .await
            .map(|record| record.map(|record| manager_record(record, Uuid::nil())))
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn save_plugin(&self, record: &InstallPluginRecord) -> Result<(), PluginManagerError> {
        self.storage
            .plugins()
            .upsert(&storage_record(record))
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn delete_plugin(&self, plugin_id: &PluginId) -> Result<(), PluginManagerError> {
        self.storage
            .plugins()
            .delete(plugin_id)
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }

    async fn persist_runtime_log(
        &self,
        record: &PluginRuntimeLogRecord,
    ) -> Result<(), PluginManagerError> {
        self.storage
            .logs()
            .append(&StructuredLog {
                id: Uuid::new_v4(),
                timestamp: record.timestamp,
                level: parse_log_level(&record.level),
                component: "plugin-runtime".to_string(),
                message: record.message.clone(),
                plugin_id: Some(record.plugin_id.to_string()),
                plugin_version: non_empty(&record.plugin_version),
                platform_id: non_empty(&record.platform_id),
                request_id: None,
                task_id: None,
                container_id: record.container_id.clone(),
                error_code: None,
                context: record.context.clone(),
            })
            .await
            .map_err(|_| PluginManagerError::PluginStateUnavailable)
    }
}

pub struct UnavailablePluginStateStore;

#[async_trait]
impl PluginStateStore for UnavailablePluginStateStore {
    async fn is_installed(&self, _plugin_id: &PluginId) -> Result<bool, PluginManagerError> {
        Err(PluginManagerError::PluginStateUnavailable)
    }
}

pub struct UnavailableRepositorySource;

#[async_trait]
impl RepositorySource for UnavailableRepositorySource {
    async fn resolve_and_download(
        &self,
        _source: &audiodown_plugin_manager::github::GitHubRepositoryRef,
        _destination: &std::path::Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError> {
        Err(PluginManagerError::RepositoryRequest)
    }
}

pub struct SupervisorPluginRuntime {
    client: std::sync::Arc<dyn SupervisorClient>,
}

impl SupervisorPluginRuntime {
    pub fn new(client: std::sync::Arc<dyn SupervisorClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl PluginRuntimeControl for SupervisorPluginRuntime {
    async fn start(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.client
            .start_plugin(plugin_id)
            .await
            .map_err(runtime_error)
    }

    async fn stop(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, PluginManagerError> {
        self.client
            .stop_plugin(plugin_id)
            .await
            .map_err(runtime_error)
    }

    async fn inspect(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, PluginManagerError> {
        self.client
            .inspect_plugin(plugin_id)
            .await
            .map_err(runtime_error)
    }

    async fn remove(&self, plugin_id: &PluginId) -> Result<PluginRemoveResult, PluginManagerError> {
        self.client
            .remove_plugin(plugin_id)
            .await
            .map_err(runtime_error)
    }

    async fn begin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.client
            .begin_plugin_install(plugin_id, operation_id)
            .await
            .map_err(runtime_error)
    }

    async fn install_status(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.client
            .plugin_install_status(plugin_id, operation_id)
            .await
            .map_err(runtime_error)
    }

    async fn finalize_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.client
            .finalize_plugin_install(plugin_id, operation_id)
            .await
            .map_err(runtime_error)
    }

    async fn abort_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.client
            .abort_plugin_install(plugin_id, operation_id)
            .await
            .map_err(runtime_error)
    }

    async fn list_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, PluginManagerError> {
        self.client
            .list_plugin_install_operations()
            .await
            .map_err(runtime_error)
    }

    async fn acknowledge_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, PluginManagerError> {
        self.client
            .acknowledge_plugin_install(plugin_id, operation_id)
            .await
            .map_err(runtime_error)
    }
}

pub struct ConfiguredLifecycleRiskAuthorizer {
    development: DevelopmentConfig,
}

impl ConfiguredLifecycleRiskAuthorizer {
    pub fn new(development: DevelopmentConfig) -> Self {
        Self { development }
    }
}

#[async_trait]
impl LifecycleRiskAuthorizer for ConfiguredLifecycleRiskAuthorizer {
    async fn authorize(
        &self,
        token: Option<&SecretString>,
    ) -> Result<(), LifecycleAuthorizationError> {
        if !self.development.enabled {
            return Err(LifecycleAuthorizationError::DeveloperModeRequired);
        }
        let (Some(expected), Some(supplied)) = (self.development.token.as_ref(), token) else {
            return Err(LifecycleAuthorizationError::TokenRequired);
        };
        if !secret_matches(expected, supplied.expose_secret()) {
            return Err(LifecycleAuthorizationError::TokenRequired);
        }
        Ok(())
    }
}

pub fn secret_matches(expected: &SecretString, supplied: &str) -> bool {
    let expected_digest = Sha256::digest(expected.expose_secret().as_bytes());
    let supplied_digest = Sha256::digest(supplied.as_bytes());
    bool::from(expected_digest.as_slice().ct_eq(supplied_digest.as_slice()))
}

fn storage_record(record: &InstallPluginRecord) -> PluginRecord {
    PluginRecord {
        plugin_id: record.plugin_id.clone(),
        plugin_type: record.plugin_type,
        platform_id: record.platform_id.clone(),
        name: record.name.clone(),
        version: record.version.clone(),
        protocol_version: record.protocol_version.clone(),
        source_kind: "github".to_string(),
        source_ref: record.source_ref.clone(),
        commit_sha: Some(record.commit_sha.clone()),
        repository_id: Some(record.repository_id.clone()),
        manifest_json: record.manifest_json.clone(),
        manifest_hash: record.manifest_hash.clone(),
        source_hash: Some(record.source_hash.clone()),
        image_id: record.image_id.clone(),
        status: record.status,
        run_mode: record.run_mode,
        priority: record.priority,
        enabled: record.enabled,
        last_error: record.last_error.clone(),
        install_operation_id: record.install_operation_id,
        last_used_at: record.last_used_at,
        installed_at: record.installed_at,
        updated_at: record.updated_at,
    }
}

fn manager_record(record: PluginRecord, operation_id: Uuid) -> InstallPluginRecord {
    InstallPluginRecord {
        operation_id,
        plugin_id: record.plugin_id,
        plugin_type: record.plugin_type,
        platform_id: record.platform_id,
        name: record.name,
        version: record.version,
        protocol_version: record.protocol_version,
        source_ref: record.source_ref,
        commit_sha: record.commit_sha.unwrap_or_default(),
        repository_id: record.repository_id.unwrap_or_default(),
        manifest_json: record.manifest_json,
        manifest_hash: record.manifest_hash,
        source_hash: record.source_hash.unwrap_or_default(),
        image_id: record.image_id,
        status: record.status,
        run_mode: record.run_mode,
        priority: record.priority,
        enabled: record.enabled,
        last_error: record.last_error,
        install_operation_id: record.install_operation_id,
        last_used_at: record.last_used_at,
        installed_at: record.installed_at,
        updated_at: record.updated_at,
    }
}

fn runtime_error(_error: SupervisorError) -> PluginManagerError {
    PluginManagerError::RuntimeUnavailable
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_log_level(level: &str) -> LogLevel {
    match level {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}
