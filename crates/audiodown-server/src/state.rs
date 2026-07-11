use std::sync::Arc;

use audiodown_storage::Storage;
use semver::Version;

pub use crate::supervisor::{
    SupervisorClient, SupervisorError, SupervisorHealth, UnavailableSupervisorClient,
};

#[derive(Clone)]
pub struct AppState {
    pub storage: Storage,
    pub core_version: Version,
    pub supervisor: Arc<dyn SupervisorClient>,
    pub development: DevelopmentConfig,
}

impl AppState {
    pub fn new(
        storage: Storage,
        core_version: Version,
        supervisor: Arc<dyn SupervisorClient>,
    ) -> Self {
        Self {
            storage,
            core_version,
            supervisor,
            development: DevelopmentConfig::default(),
        }
    }

    pub fn with_development(mut self, enabled: bool, token: Option<String>) -> Self {
        self.development = DevelopmentConfig { enabled, token };
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct DevelopmentConfig {
    pub enabled: bool,
    pub token: Option<String>,
}
