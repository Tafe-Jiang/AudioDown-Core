use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::content::ContentMethod;
pub use audiodown_supervisor_protocol::{
    PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact, PluginInstallOperation,
    PluginInstallOperationList, PluginInstallOperationState, PluginInstallOperationSummary,
    PluginRemoveResult, PluginRpcResult, PluginRuntimeLog, PluginRuntimeState, SupervisorHealth,
};
use audiodown_supervisor_protocol::{
    PluginInstallRequest, PluginRequest, PluginRpcRequest, SupervisorMethod, SupervisorParams,
    SupervisorRequest, SupervisorResponse,
};
use chrono::Utc;
use serde::{de::DeserializeOwned, Deserialize};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use uuid::Uuid;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
const PLUGIN_RPC_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const PLUGIN_INSTALL_POLL_INTERVAL: Duration = Duration::from_millis(500);
pub const PLUGIN_INSTALL_WAIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[async_trait]
pub trait SupervisorClient: Send + Sync {
    async fn ping(&self) -> Result<SupervisorHealth, SupervisorError>;
    async fn start_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError>;
    async fn stop_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError>;
    async fn inspect_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError>;
    async fn invoke_plugin(
        &self,
        plugin_id: &PluginId,
        method: ContentMethod,
        params: serde_json::Value,
    ) -> Result<PluginRpcResult, SupervisorError>;
    async fn begin_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError>;
    async fn plugin_install_status(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError>;
    async fn finalize_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError>;
    async fn abort_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError>;
    async fn list_plugin_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, SupervisorError>;
    async fn acknowledge_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError>;
    async fn remove_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRemoveResult, SupervisorError>;
}

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("Supervisor is unavailable")]
    Unavailable,
    #[error("Supervisor request timed out")]
    Timeout,
    #[error("Supervisor returned an invalid response")]
    InvalidResponse,
    #[error("Supervisor response exceeded the size limit")]
    ResponseTooLarge,
    #[error("Supervisor rejected the request with code {code}")]
    Protocol { code: String },
}

impl SupervisorError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable | Self::Timeout)
    }
}

pub struct UnixSupervisorClient {
    socket_path: PathBuf,
    token_path: PathBuf,
    timeout: Duration,
    max_response_bytes: usize,
}

impl UnixSupervisorClient {
    pub fn new(socket_path: impl AsRef<Path>, token_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            token_path: token_path.as_ref().to_path_buf(),
            timeout: DEFAULT_TIMEOUT,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: SupervisorMethod,
        params: Option<SupervisorParams>,
    ) -> Result<T, SupervisorError> {
        tokio::time::timeout(self.timeout, self.exchange(method, params))
            .await
            .map_err(|_| SupervisorError::Timeout)?
    }

    async fn call_with_timeout<T: DeserializeOwned>(
        &self,
        timeout: Duration,
        method: SupervisorMethod,
        params: Option<SupervisorParams>,
    ) -> Result<T, SupervisorError> {
        tokio::time::timeout(timeout, self.exchange(method, params))
            .await
            .map_err(|_| SupervisorError::Timeout)?
    }

    async fn exchange<T: DeserializeOwned>(
        &self,
        method: SupervisorMethod,
        params: Option<SupervisorParams>,
    ) -> Result<T, SupervisorError> {
        let token = tokio::fs::read_to_string(&self.token_path)
            .await
            .map_err(|_| SupervisorError::Unavailable)?;
        let token = token.trim();
        if token.is_empty() {
            return Err(SupervisorError::Unavailable);
        }

        let request = SupervisorRequest {
            id: Uuid::new_v4().to_string(),
            token: token.to_string(),
            timestamp: Utc::now().timestamp(),
            nonce: Uuid::new_v4().to_string(),
            method,
            params,
        };
        let mut encoded =
            serde_json::to_vec(&request).map_err(|_| SupervisorError::InvalidResponse)?;
        encoded.push(b'\n');

        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|_| SupervisorError::Unavailable)?;
        stream
            .write_all(&encoded)
            .await
            .map_err(|_| SupervisorError::Unavailable)?;

        let response_bytes = read_response(&mut stream, self.max_response_bytes).await?;
        let response: SupervisorResponse = serde_json::from_slice(&response_bytes)
            .map_err(|_| SupervisorError::InvalidResponse)?;
        if !response.ok {
            let code = response
                .error
                .map(|error| error.code)
                .unwrap_or_else(|| "UNKNOWN_SUPERVISOR_ERROR".to_string());
            return Err(SupervisorError::Protocol { code });
        }
        let result = response.result.ok_or(SupervisorError::InvalidResponse)?;
        serde_json::from_value(result).map_err(|_| SupervisorError::InvalidResponse)
    }
}

#[async_trait]
impl SupervisorClient for UnixSupervisorClient {
    async fn ping(&self) -> Result<SupervisorHealth, SupervisorError> {
        #[derive(Deserialize)]
        struct PingResult {
            ok: bool,
            service: String,
        }

        let result: PingResult = self.call(SupervisorMethod::SystemPing, None).await?;
        if !result.ok {
            return Err(SupervisorError::InvalidResponse);
        }
        Ok(SupervisorHealth {
            service: result.service,
        })
    }

    async fn start_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        self.call(
            SupervisorMethod::PluginStart,
            Some(plugin_params(plugin_id)),
        )
        .await
    }

    async fn stop_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        self.call(SupervisorMethod::PluginStop, Some(plugin_params(plugin_id)))
            .await
    }

    async fn inspect_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        self.call(
            SupervisorMethod::PluginInspect,
            Some(plugin_params(plugin_id)),
        )
        .await
    }

    async fn invoke_plugin(
        &self,
        plugin_id: &PluginId,
        method: ContentMethod,
        params: serde_json::Value,
    ) -> Result<PluginRpcResult, SupervisorError> {
        let result: PluginRpcResult = self
            .call_with_timeout(
                PLUGIN_RPC_TIMEOUT,
                SupervisorMethod::PluginRpc,
                Some(SupervisorParams::Rpc(PluginRpcRequest {
                    plugin_id: plugin_id.clone(),
                    method,
                    params,
                })),
            )
            .await?;
        result
            .validate()
            .map_err(|_| SupervisorError::InvalidResponse)?;
        Ok(result)
    }

    async fn begin_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        self.call(
            SupervisorMethod::PluginInstallBuild,
            Some(install_params(plugin_id, operation_id)),
        )
        .await
    }

    async fn plugin_install_status(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        self.call(
            SupervisorMethod::PluginInstallStatus,
            Some(install_params(plugin_id, operation_id)),
        )
        .await
    }

    async fn finalize_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        self.call(
            SupervisorMethod::PluginInstallFinalize,
            Some(install_params(plugin_id, operation_id)),
        )
        .await
    }

    async fn abort_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        self.call(
            SupervisorMethod::PluginInstallAbort,
            Some(install_params(plugin_id, operation_id)),
        )
        .await
    }

    async fn list_plugin_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, SupervisorError> {
        self.call(SupervisorMethod::PluginInstallList, None).await
    }

    async fn acknowledge_plugin_install(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        self.call(
            SupervisorMethod::PluginInstallAck,
            Some(install_params(plugin_id, operation_id)),
        )
        .await
    }

    async fn remove_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRemoveResult, SupervisorError> {
        self.call(
            SupervisorMethod::PluginRemove,
            Some(plugin_params(plugin_id)),
        )
        .await
    }
}

pub struct UnavailableSupervisorClient;

#[async_trait]
impl SupervisorClient for UnavailableSupervisorClient {
    async fn ping(&self) -> Result<SupervisorHealth, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn start_plugin(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn stop_plugin(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn inspect_plugin(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn invoke_plugin(
        &self,
        _plugin_id: &PluginId,
        _method: ContentMethod,
        _params: serde_json::Value,
    ) -> Result<PluginRpcResult, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn begin_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn plugin_install_status(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn finalize_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn abort_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn list_plugin_install_operations(
        &self,
    ) -> Result<PluginInstallOperationList, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn acknowledge_plugin_install(
        &self,
        _plugin_id: &PluginId,
        _operation_id: Uuid,
    ) -> Result<PluginInstallOperation, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }

    async fn remove_plugin(
        &self,
        _plugin_id: &PluginId,
    ) -> Result<PluginRemoveResult, SupervisorError> {
        Err(SupervisorError::Unavailable)
    }
}

async fn read_response(
    stream: &mut UnixStream,
    max_bytes: usize,
) -> Result<Vec<u8>, SupervisorError> {
    let mut response = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    loop {
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|_| SupervisorError::Unavailable)?;
        if read == 0 || byte[0] == b'\n' {
            break;
        }
        response.push(byte[0]);
        if response.len() > max_bytes {
            return Err(SupervisorError::ResponseTooLarge);
        }
    }
    if response.is_empty() {
        return Err(SupervisorError::InvalidResponse);
    }
    Ok(response)
}

fn plugin_params(plugin_id: &PluginId) -> SupervisorParams {
    SupervisorParams::Plugin(PluginRequest {
        plugin_id: plugin_id.clone(),
    })
}

fn install_params(plugin_id: &PluginId, operation_id: Uuid) -> SupervisorParams {
    SupervisorParams::Install(PluginInstallRequest {
        plugin_id: plugin_id.clone(),
        operation_id,
    })
}
