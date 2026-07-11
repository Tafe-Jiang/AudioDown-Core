use std::sync::Arc;

use audiodown_plugin_manager::service::PluginManagerService;
use audiodown_storage::Storage;
use secrecy::SecretString;
use semver::Version;

use crate::content_adapters::ContentApiService;
use crate::plugin_manager_adapters::{UnavailablePluginStateStore, UnavailableRepositorySource};

pub use crate::supervisor::{
    SupervisorClient, SupervisorError, SupervisorHealth, UnavailableSupervisorClient,
};

#[derive(Clone)]
pub struct AppState {
    pub storage: Storage,
    pub core_version: Version,
    pub supervisor: Arc<dyn SupervisorClient>,
    pub plugin_manager: Arc<PluginManagerService>,
    pub content: Arc<ContentApiService>,
    pub development: DevelopmentConfig,
}

impl AppState {
    pub fn new(
        storage: Storage,
        core_version: Version,
        supervisor: Arc<dyn SupervisorClient>,
    ) -> Self {
        let plugin_manager = Arc::new(PluginManagerService::new(
            Arc::new(UnavailablePluginStateStore),
            Arc::new(UnavailableRepositorySource),
            std::env::temp_dir().join("audiodown-unavailable-plugin-manager"),
            core_version.clone(),
            Version::new(1, 0, 0),
        ));
        let content = Arc::new(ContentApiService::new(
            storage.clone(),
            Arc::clone(&plugin_manager),
        ));
        Self {
            storage,
            core_version,
            supervisor,
            plugin_manager,
            content,
            development: DevelopmentConfig::default(),
        }
    }

    pub fn with_plugin_manager(mut self, plugin_manager: Arc<PluginManagerService>) -> Self {
        self.content = Arc::new(ContentApiService::new(
            self.storage.clone(),
            Arc::clone(&plugin_manager),
        ));
        self.plugin_manager = plugin_manager;
        self
    }

    pub fn with_development(mut self, enabled: bool, token: Option<SecretString>) -> Self {
        self.development = DevelopmentConfig { enabled, token };
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct DevelopmentConfig {
    pub enabled: bool,
    pub token: Option<SecretString>,
}

impl DevelopmentConfig {
    pub fn public_view(&self) -> serde_json::Value {
        serde_json::json!({"developmentMode": self.enabled})
    }
}
