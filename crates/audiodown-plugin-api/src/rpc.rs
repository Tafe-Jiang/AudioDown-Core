use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

impl JsonRpcRequest {
    pub fn new<T: Serialize>(
        id: impl Into<String>,
        method: impl Into<String>,
        params: T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params: serde_json::to_value(params)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHello {
    pub protocol_version: String,
    pub core_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHelloResult {
    pub plugin_id: String,
    pub plugin_version: String,
    pub protocol_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHealthResult {
    pub healthy: bool,
    pub uptime_seconds: u64,
}
