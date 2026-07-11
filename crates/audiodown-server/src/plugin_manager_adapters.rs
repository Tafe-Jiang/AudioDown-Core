use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_manager::{
    service::PluginStateStore, DownloadedSnapshot, PluginManagerError, RepositorySource,
};
use audiodown_storage::Storage;

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
