use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use audiodown_domain::plugin::{PluginId, PluginStatus};
use chrono::Utc;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use uuid::Uuid;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorHealth {
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeState {
    pub plugin_id: PluginId,
    pub status: PluginStatus,
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
        method: &'static str,
        plugin_id: Option<&PluginId>,
    ) -> Result<T, SupervisorError> {
        tokio::time::timeout(self.timeout, self.exchange(method, plugin_id))
            .await
            .map_err(|_| SupervisorError::Timeout)?
    }

    async fn exchange<T: DeserializeOwned>(
        &self,
        method: &'static str,
        plugin_id: Option<&PluginId>,
    ) -> Result<T, SupervisorError> {
        let token = tokio::fs::read_to_string(&self.token_path)
            .await
            .map_err(|_| SupervisorError::Unavailable)?;
        let token = token.trim();
        if token.is_empty() {
            return Err(SupervisorError::Unavailable);
        }

        let request = WireRequest {
            id: Uuid::new_v4().to_string(),
            token,
            timestamp: Utc::now().timestamp(),
            nonce: Uuid::new_v4().to_string(),
            method,
            params: plugin_id.map(|plugin_id| WirePluginRequest { plugin_id }),
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
        let response: WireResponse =
            serde_json::from_slice(&response_bytes).map_err(|_| SupervisorError::InvalidResponse)?;
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

        let result: PingResult = self.call("system.ping", None).await?;
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
        self.call("plugin.start", Some(plugin_id)).await
    }

    async fn stop_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        self.call("plugin.stop", Some(plugin_id)).await
    }

    async fn inspect_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginRuntimeState, SupervisorError> {
        self.call("plugin.inspect", Some(plugin_id)).await
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

#[derive(Serialize)]
struct WireRequest<'a> {
    id: String,
    token: &'a str,
    timestamp: i64,
    nonce: String,
    method: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<WirePluginRequest<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WirePluginRequest<'a> {
    plugin_id: &'a PluginId,
}

#[derive(Deserialize)]
struct WireResponse {
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<WireError>,
}

#[derive(Deserialize)]
struct WireError {
    code: String,
}
