use audiodown_domain::credential::CredentialScope;
use audiodown_domain::plugin::PluginId;
use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::content::ContentMethod;
use crate::credential::CredentialMethod;

pub const SYSTEM_HEALTH_CAPABILITY: &str = "system.health";
pub const MAX_CREDENTIAL_SCOPE_DECLARATIONS: usize = 32;
pub const MAX_CREDENTIAL_TARGET_ORIGINS: usize = 16;
pub const MAX_CREDENTIAL_TARGET_ORIGIN_BYTES: usize = 2 * 1024;

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
    pub credentials: CredentialDeclarations,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialDeclarations {
    #[serde(default)]
    pub provided_scopes: Vec<CredentialScopeDeclaration>,
    #[serde(default)]
    pub required_scopes: Vec<CredentialScopeDeclaration>,
    #[serde(default)]
    pub optional_scopes: Vec<CredentialScopeDeclaration>,
}

impl CredentialDeclarations {
    pub fn is_empty(&self) -> bool {
        self.provided_scopes.is_empty()
            && self.required_scopes.is_empty()
            && self.optional_scopes.is_empty()
    }

    pub fn declaration_count(&self) -> usize {
        self.provided_scopes.len() + self.required_scopes.len() + self.optional_scopes.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialScopeDeclaration {
    pub scope: CredentialScope,
    pub target_origins: Vec<CredentialTargetOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CredentialTargetOrigin(String);

impl CredentialTargetOrigin {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, CredentialTargetOriginError> {
        let value = value.as_ref();
        if value.is_empty()
            || value != value.trim()
            || value.len() > MAX_CREDENTIAL_TARGET_ORIGIN_BYTES
        {
            return Err(CredentialTargetOriginError::Invalid);
        }

        let parsed = Url::parse(value).map_err(|_| CredentialTargetOriginError::Invalid)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.host_str().is_none()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.host_str().is_some_and(|host| host.contains('*'))
        {
            return Err(CredentialTargetOriginError::Invalid);
        }

        let normalized = parsed.origin().ascii_serialization();
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CredentialTargetOrigin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for CredentialTargetOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CredentialTargetOriginError {
    #[error("credential target origin must be an exact HTTP origin")]
    Invalid,
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
