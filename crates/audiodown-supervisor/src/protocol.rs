use std::sync::Mutex;

use audiodown_domain::plugin::PluginId;
use audiodown_supervisor_protocol::{
    PluginInstallOperation, PluginInstallOperationList, PluginInstallOperationState,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub use audiodown_supervisor_protocol::{
    PluginInstallRequest, PluginRequest, PluginRpcRequest, ProtocolError, SupervisorMethod,
    SupervisorParams, SupervisorProtocolError, SupervisorRequest, SupervisorResponse,
};

#[derive(Default)]
pub struct ProtocolOperationStore {
    operations: Mutex<Vec<OwnedOperation>>,
}

impl ProtocolOperationStore {
    pub fn begin(
        &self,
        installation_id: &str,
        plugin_id: PluginId,
        operation_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, OperationStoreError> {
        let mut operations = self.lock()?;
        if let Some(existing) = operations.iter().find(|owned| {
            owned.installation_id == installation_id && owned.operation.operation_id == operation_id
        }) {
            if existing.operation.plugin_id != plugin_id {
                return Err(OperationStoreError::OperationIdMismatch);
            }
            return Ok(existing.operation.clone());
        }

        let operation = PluginInstallOperation {
            operation_id,
            plugin_id,
            state: PluginInstallOperationState::Accepted,
            artifact: None,
            build_logs: Vec::new(),
            error_code: None,
            acknowledged: false,
        };
        operations.push(OwnedOperation {
            installation_id: installation_id.to_string(),
            operation: operation.clone(),
            updated_at: now,
        });
        Ok(operation)
    }

    pub fn get(
        &self,
        installation_id: &str,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, OperationStoreError> {
        let operations = self.lock()?;
        operations
            .iter()
            .find(|owned| {
                owned.installation_id == installation_id
                    && owned.operation.operation_id == operation_id
                    && &owned.operation.plugin_id == plugin_id
            })
            .map(|owned| owned.operation.clone())
            .ok_or(OperationStoreError::NotFound)
    }

    pub fn set_state(
        &self,
        installation_id: &str,
        plugin_id: &PluginId,
        operation_id: Uuid,
        state: PluginInstallOperationState,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, OperationStoreError> {
        let mut operations = self.lock()?;
        let owned = operations
            .iter_mut()
            .find(|owned| {
                owned.installation_id == installation_id
                    && owned.operation.operation_id == operation_id
                    && &owned.operation.plugin_id == plugin_id
            })
            .ok_or(OperationStoreError::NotFound)?;
        owned.operation.state = state;
        owned.updated_at = now;
        Ok(owned.operation.clone())
    }

    pub fn list(
        &self,
        installation_id: &str,
    ) -> Result<PluginInstallOperationList, OperationStoreError> {
        let operations = self.lock()?;
        Ok(PluginInstallOperationList::new(
            operations
                .iter()
                .filter(|owned| {
                    owned.installation_id == installation_id && !owned.operation.acknowledged
                })
                .map(|owned| owned.operation.summary())
                .collect(),
        ))
    }

    pub fn acknowledge(
        &self,
        installation_id: &str,
        plugin_id: &PluginId,
        operation_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<PluginInstallOperation, OperationStoreError> {
        let mut operations = self.lock()?;
        let owned = operations
            .iter_mut()
            .find(|owned| {
                owned.installation_id == installation_id
                    && owned.operation.operation_id == operation_id
                    && &owned.operation.plugin_id == plugin_id
            })
            .ok_or(OperationStoreError::NotFound)?;
        if !owned.operation.state.is_terminal() {
            return Err(OperationStoreError::NotTerminal);
        }
        owned.operation.acknowledged = true;
        owned.updated_at = now;
        Ok(owned.operation.clone())
    }

    pub fn cleanup_acknowledged_before(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<(), OperationStoreError> {
        let mut operations = self.lock()?;
        operations.retain(|owned| {
            !owned.operation.acknowledged
                || !owned.operation.state.is_terminal()
                || owned.updated_at >= cutoff
        });
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Vec<OwnedOperation>>, OperationStoreError> {
        self.operations
            .lock()
            .map_err(|_| OperationStoreError::StatePoisoned)
    }
}

struct OwnedOperation {
    installation_id: String,
    operation: PluginInstallOperation,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OperationStoreError {
    #[error("operation ID belongs to another plugin")]
    OperationIdMismatch,
    #[error("operation was not found")]
    NotFound,
    #[error("operation is not terminal")]
    NotTerminal,
    #[error("operation state is unavailable")]
    StatePoisoned,
}
