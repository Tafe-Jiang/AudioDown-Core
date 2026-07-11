use std::sync::Arc;

use async_trait::async_trait;
use audiodown_storage::Storage;
use semver::Version;
use serde::Serialize;
use thiserror::Error;

#[derive(Clone)]
pub struct AppState {
    pub storage: Storage,
    pub core_version: Version,
    pub supervisor: Arc<dyn SupervisorClient>,
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
        }
    }
}

#[async_trait]
pub trait SupervisorClient: Send + Sync {
    async fn ping(&self) -> Result<SupervisorHealth, SupervisorError>;
}

#[derive(Debug, Clone, Serialize)]
pub struct SupervisorHealth {
    pub service: String,
}

#[derive(Debug, Error)]
#[error("{summary}")]
pub struct SupervisorError {
    pub summary: String,
}

pub struct UnavailableSupervisorClient;

#[async_trait]
impl SupervisorClient for UnavailableSupervisorClient {
    async fn ping(&self) -> Result<SupervisorHealth, SupervisorError> {
        Err(SupervisorError {
            summary: "Supervisor is unavailable".to_string(),
        })
    }
}
