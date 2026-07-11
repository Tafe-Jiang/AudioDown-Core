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
