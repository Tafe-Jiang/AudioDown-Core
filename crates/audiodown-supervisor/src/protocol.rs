use audiodown_domain::plugin::PluginId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorRequest {
    pub id: String,
    pub token: String,
    pub timestamp: i64,
    pub nonce: String,
    pub method: SupervisorMethod,
    #[serde(default)]
    pub params: Option<PluginRequest>,
}

impl SupervisorRequest {
    pub fn validate_shape(&self) -> Result<(), ProtocolError> {
        match self.method {
            SupervisorMethod::SystemPing if self.params.is_none() => Ok(()),
            SupervisorMethod::SystemPing => Err(ProtocolError::UnexpectedParams),
            _ if self.params.is_some() => Ok(()),
            _ => Err(ProtocolError::MissingParams),
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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRequest {
    pub plugin_id: PluginId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("request parameters are required")]
    MissingParams,
    #[error("request parameters are not allowed")]
    UnexpectedParams,
}
