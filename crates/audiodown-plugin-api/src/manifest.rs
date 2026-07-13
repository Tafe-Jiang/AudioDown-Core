use audiodown_domain::plugin::PluginId;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::content::ContentMethod;
use crate::credential::CredentialMethod;

pub const SYSTEM_HEALTH_CAPABILITY: &str = "system.health";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    pub id: PluginId,
    pub name: String,
    pub version: Version,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    pub runtime: RuntimeSpec,
    pub compatibility: CompatibilitySpec,
    pub platform: PlatformSpec,
    pub capabilities: Vec<String>,
    pub network: NetworkPolicy,
    #[serde(default)]
    pub build: BuildSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    Content,
    Credential,
}

pub fn capability_is_supported(plugin_type: PluginType, capability: &str) -> bool {
    capability == SYSTEM_HEALTH_CAPABILITY
        || (plugin_type == PluginType::Content
            && ContentMethod::from_capability(capability).is_some())
        || (plugin_type == PluginType::Credential
            && CredentialMethod::from_capability(capability).is_some())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSpec {
    #[serde(rename = "type")]
    pub kind: RuntimeKind,
    pub version: String,
    pub entry: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Nodejs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilitySpec {
    #[serde(rename = "pluginApi")]
    pub plugin_api: String,
    pub core: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSpec {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicy {
    #[serde(rename = "allowedHosts")]
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildSpec {
    #[serde(default)]
    pub npm_lifecycle_scripts: LifecycleScriptPolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleScriptPolicy {
    #[serde(default)]
    pub required: bool,
    pub reason: Option<String>,
}
