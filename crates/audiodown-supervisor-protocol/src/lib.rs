#![forbid(unsafe_code)]

use audiodown_domain::plugin::{PluginId, PluginStatus};
use audiodown_plugin_api::{
    content::{
        AlbumGetRequest, CategoriesRequest, ContentMethod, DiscoverRequest, SearchRequest,
        TracksListRequest,
    },
    rpc::JsonRpcResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_OPERATION_LIST_ITEMS: usize = 256;
const MAX_ERROR_DETAILS_BYTES: usize = 64 * 1024;
const MAX_PLUGIN_RPC_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorRequest {
    pub id: String,
    pub token: String,
    pub timestamp: i64,
    pub nonce: String,
    pub method: SupervisorMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<SupervisorParams>,
}

impl SupervisorRequest {
    pub fn validate_shape(&self) -> Result<Option<&SupervisorParams>, ProtocolError> {
        match self.method {
            SupervisorMethod::SystemPing | SupervisorMethod::PluginInstallList => {
                if self.params.is_none() {
                    Ok(None)
                } else {
                    Err(ProtocolError::UnexpectedParams)
                }
            }
            SupervisorMethod::PluginInstallBuild
            | SupervisorMethod::PluginInstallStatus
            | SupervisorMethod::PluginInstallFinalize
            | SupervisorMethod::PluginInstallAbort
            | SupervisorMethod::PluginInstallAck => match self.params.as_ref() {
                Some(params @ SupervisorParams::Install(_)) => Ok(Some(params)),
                Some(_) => Err(ProtocolError::InvalidParams),
                None => Err(ProtocolError::MissingParams),
            },
            SupervisorMethod::PluginStart
            | SupervisorMethod::PluginStop
            | SupervisorMethod::PluginInspect
            | SupervisorMethod::PluginLogs
            | SupervisorMethod::PluginRemove => match self.params.as_ref() {
                Some(params @ SupervisorParams::Plugin(_)) => Ok(Some(params)),
                Some(_) => Err(ProtocolError::InvalidParams),
                None => Err(ProtocolError::MissingParams),
            },
            SupervisorMethod::PluginRpc => match self.params.as_ref() {
                Some(params @ SupervisorParams::Rpc(request)) => {
                    request.validate()?;
                    Ok(Some(params))
                }
                Some(_) => Err(ProtocolError::InvalidParams),
                None => Err(ProtocolError::MissingParams),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum SupervisorMethod {
    #[serde(rename = "system.ping")]
    SystemPing,
    #[serde(rename = "plugin.start")]
    PluginStart,
    #[serde(rename = "plugin.stop")]
    PluginStop,
    #[serde(rename = "plugin.inspect")]
    PluginInspect,
    #[serde(rename = "plugin.logs")]
    PluginLogs,
    #[serde(rename = "plugin.rpc")]
    PluginRpc,
    #[serde(rename = "plugin.install.build")]
    PluginInstallBuild,
    #[serde(rename = "plugin.install.status")]
    PluginInstallStatus,
    #[serde(rename = "plugin.install.finalize")]
    PluginInstallFinalize,
    #[serde(rename = "plugin.install.abort")]
    PluginInstallAbort,
    #[serde(rename = "plugin.install.list")]
    PluginInstallList,
    #[serde(rename = "plugin.install.ack")]
    PluginInstallAck,
    #[serde(rename = "plugin.remove")]
    PluginRemove,
}

impl SupervisorMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemPing => "system.ping",
            Self::PluginStart => "plugin.start",
            Self::PluginStop => "plugin.stop",
            Self::PluginInspect => "plugin.inspect",
            Self::PluginLogs => "plugin.logs",
            Self::PluginRpc => "plugin.rpc",
            Self::PluginInstallBuild => "plugin.install.build",
            Self::PluginInstallStatus => "plugin.install.status",
            Self::PluginInstallFinalize => "plugin.install.finalize",
            Self::PluginInstallAbort => "plugin.install.abort",
            Self::PluginInstallList => "plugin.install.list",
            Self::PluginInstallAck => "plugin.install.ack",
            Self::PluginRemove => "plugin.remove",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SupervisorParams {
    Install(PluginInstallRequest),
    Plugin(PluginRequest),
    Rpc(PluginRpcRequest),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRequest {
    pub plugin_id: PluginId,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallRequest {
    pub plugin_id: PluginId,
    pub operation_id: Uuid,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRpcRequest {
    pub plugin_id: PluginId,
    pub method: ContentMethod,
    pub params: serde_json::Value,
}

impl PluginRpcRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let encoded =
            serde_json::to_vec(&self.params).map_err(|_| ProtocolError::InvalidRpcParams)?;
        if encoded.len() > MAX_PLUGIN_RPC_BYTES {
            return Err(ProtocolError::RpcParamsTooLarge);
        }

        let valid = match self.method {
            ContentMethod::Search => serde_json::from_value::<SearchRequest>(self.params.clone())
                .map_err(|_| ProtocolError::InvalidRpcParams)?
                .validate(),
            ContentMethod::Discover => {
                serde_json::from_value::<DiscoverRequest>(self.params.clone())
                    .map_err(|_| ProtocolError::InvalidRpcParams)?
                    .validate()
            }
            ContentMethod::Categories => {
                serde_json::from_value::<CategoriesRequest>(self.params.clone())
                    .map_err(|_| ProtocolError::InvalidRpcParams)?;
                Ok(())
            }
            ContentMethod::AlbumGet => {
                serde_json::from_value::<AlbumGetRequest>(self.params.clone())
                    .map_err(|_| ProtocolError::InvalidRpcParams)?
                    .validate()
            }
            ContentMethod::TracksList => {
                serde_json::from_value::<TracksListRequest>(self.params.clone())
                    .map_err(|_| ProtocolError::InvalidRpcParams)?
                    .validate()
            }
        };
        valid.map_err(|_| ProtocolError::InvalidRpcParams)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRpcResult {
    pub response: JsonRpcResponse,
}

impl PluginRpcResult {
    pub fn new(response: JsonRpcResponse) -> Result<Self, ProtocolError> {
        let result = Self { response };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        let encoded =
            serde_json::to_vec(&self.response).map_err(|_| ProtocolError::InvalidRpcResponse)?;
        if encoded.len() > MAX_PLUGIN_RPC_BYTES {
            return Err(ProtocolError::RpcResponseTooLarge);
        }
        if self.response.jsonrpc != "2.0"
            || self.response.id.is_empty()
            || self.response.id.len() > 128
            || self.response.result.is_some() == self.response.error.is_some()
        {
            return Err(ProtocolError::InvalidRpcResponse);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorResponse {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SupervisorProtocolError>,
}

impl SupervisorResponse {
    pub fn success(id: impl Into<String>, result: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(SupervisorProtocolError {
                code: code.into(),
                message: message.into(),
                details: None,
            }),
        }
    }

    pub fn failure_with_details(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Result<Self, ProtocolError> {
        validate_error_details(&details)?;
        Ok(Self {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(SupervisorProtocolError {
                code: code.into(),
                message: message.into(),
                details: Some(details),
            }),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorProtocolError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupervisorHealth {
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeState {
    pub plugin_id: PluginId,
    pub status: PluginStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<PluginRuntimeLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginRuntimeLog {
    pub level: String,
    pub message: String,
    #[serde(default)]
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginInstallOperationState {
    Accepted,
    Building,
    Built,
    Finalized,
    Failed,
    Aborted,
}

impl PluginInstallOperationState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Finalized | Self::Failed | Self::Aborted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallArtifact {
    pub image_id: String,
    pub repository_id: String,
    pub commit_sha: String,
    pub source_hash: String,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginBuildLogStream {
    Stdout,
    Stderr,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginBuildLog {
    pub sequence: u64,
    pub stream: PluginBuildLogStream,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallOperation {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
    pub state: PluginInstallOperationState,
    pub artifact: Option<PluginInstallArtifact>,
    #[serde(default)]
    pub build_logs: Vec<PluginBuildLog>,
    pub error_code: Option<String>,
    pub acknowledged: bool,
}

impl PluginInstallOperation {
    pub fn summary(&self) -> PluginInstallOperationSummary {
        PluginInstallOperationSummary {
            operation_id: self.operation_id,
            plugin_id: self.plugin_id.clone(),
            state: self.state,
            artifact: self.artifact.clone(),
            error_code: self.error_code.clone(),
            acknowledged: self.acknowledged,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallOperationSummary {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
    pub state: PluginInstallOperationState,
    pub artifact: Option<PluginInstallArtifact>,
    pub error_code: Option<String>,
    pub acknowledged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallOperationList {
    pub operations: Vec<PluginInstallOperationSummary>,
}

impl PluginInstallOperationList {
    pub fn new(mut operations: Vec<PluginInstallOperationSummary>) -> Self {
        operations.truncate(MAX_OPERATION_LIST_ITEMS);
        Self { operations }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoveResult {
    pub plugin_id: PluginId,
    pub removed_container: bool,
    pub removed_image: bool,
    pub removed_install_directory: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("request parameters are required")]
    MissingParams,
    #[error("request parameters are not allowed")]
    UnexpectedParams,
    #[error("request parameters do not match the method")]
    InvalidParams,
    #[error("plugin RPC parameters are invalid")]
    InvalidRpcParams,
    #[error("plugin RPC parameters exceed the size limit")]
    RpcParamsTooLarge,
    #[error("plugin RPC response is invalid")]
    InvalidRpcResponse,
    #[error("plugin RPC response exceeds the size limit")]
    RpcResponseTooLarge,
    #[error("protocol error details may contain only buildLogs")]
    InvalidErrorDetails,
    #[error("protocol error details exceed the size limit")]
    ErrorDetailsTooLarge,
}

fn validate_error_details(details: &serde_json::Value) -> Result<(), ProtocolError> {
    let object = details
        .as_object()
        .ok_or(ProtocolError::InvalidErrorDetails)?;
    if object.len() != 1
        || !object
            .get("buildLogs")
            .is_some_and(serde_json::Value::is_array)
    {
        return Err(ProtocolError::InvalidErrorDetails);
    }
    let encoded = serde_json::to_vec(details).map_err(|_| ProtocolError::InvalidErrorDetails)?;
    if encoded.len() > MAX_ERROR_DETAILS_BYTES {
        return Err(ProtocolError::ErrorDetailsTooLarge);
    }
    Ok(())
}
