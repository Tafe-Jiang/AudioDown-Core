use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::manifest::PluginType;
use chrono::{DateTime, Utc};
use semver::Version;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    archive::{extract_snapshot, SnapshotLimits},
    github::GitHubRepositoryRef,
    staging::{PluginPreview, SnapshotStore},
    validation::validate_repository,
    PluginManagerError, RepositorySource,
};

const MAX_CONCURRENT_INSPECTIONS: usize = 2;

#[async_trait]
pub trait PluginStateStore: Send + Sync {
    async fn is_installed(&self, plugin_id: &PluginId) -> Result<bool, PluginManagerError>;
}

pub struct PluginManagerService {
    state_store: Arc<dyn PluginStateStore>,
    repository_source: Arc<dyn RepositorySource>,
    snapshots: SnapshotStore,
    core_version: Version,
    plugin_api_version: Version,
    inspection_permits: Semaphore,
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
            snapshots: SnapshotStore::new(plugin_data),
            core_version,
            plugin_api_version,
            inspection_permits: Semaphore::new(MAX_CONCURRENT_INSPECTIONS),
        }
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

fn map_download_error(error: PluginManagerError) -> InspectionError {
    match error {
        PluginManagerError::InvalidRepositoryUrl => InspectionError::InvalidRepositoryUrl,
        _ => InspectionError::RepositoryUnavailable,
    }
}

fn map_repository_error(_error: PluginManagerError) -> InspectionError {
    InspectionError::InvalidRepository
}
