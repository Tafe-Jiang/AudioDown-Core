use std::collections::HashMap;

use audiodown_domain::plugin::PluginId;
use audiodown_supervisor_protocol::ProxyToken;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const DEFAULT_GATEWAY_IMAGE: &str = "audiodown/plugin-gateway:1.0.0-alpha.1";
pub const DEFAULT_PROXY_VOLUME: &str = "audiodown-proxy";
pub const PROXY_GATEWAY_URL: &str = "http://audiodown-gateway:18081";
pub const PROXY_GATEWAY_ALIAS: &str = "audiodown-gateway";
pub const PROXY_BACKEND_SOCKET: &str = "/run/audiodown-proxy/core.sock";
const PROXY_MOUNT_PATH: &str = "/run/audiodown-proxy";
const GATEWAY_MEMORY_BYTES: i64 = 64 * 1024 * 1024;
const GATEWAY_NANO_CPUS: i64 = 250_000_000;
const GATEWAY_PIDS_LIMIT: i64 = 32;

#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub plugin_id: PluginId,
    pub image_id: String,
    pub installation_id: String,
    pub runtime_path: String,
    pub memory_bytes: i64,
    pub nano_cpus: i64,
    pub pids_limit: i64,
}

#[derive(Debug, Clone)]
pub struct PluginContainerSpec {
    pub plugin_id: PluginId,
    pub image_id: String,
    pub container_name: String,
    pub command: Vec<String>,
    pub labels: HashMap<String, String>,
    pub read_only: bool,
    pub tmpfs: Vec<String>,
    pub mounts: Vec<String>,
    pub public_ports: Vec<u16>,
    pub privileged: bool,
    pub cap_add: Vec<String>,
    pub cap_drop: Vec<String>,
    pub security_opt: Vec<String>,
    pub pids_limit: i64,
    pub memory_bytes: i64,
    pub nano_cpus: i64,
    pub host_network: bool,
}

#[derive(Debug, Clone)]
pub struct GatewayRuntimeConfig {
    pub image: String,
    pub proxy_volume: String,
}

impl GatewayRuntimeConfig {
    pub fn new(
        image: impl Into<String>,
        proxy_volume: impl Into<String>,
    ) -> Result<Self, PolicyError> {
        let image = image.into();
        let proxy_volume = proxy_volume.into();
        if image.is_empty()
            || image.len() > 512
            || !image
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-/:@".contains(&byte))
        {
            return Err(PolicyError::InvalidGatewayImage);
        }
        if proxy_volume.is_empty()
            || proxy_volume.len() > 255
            || !proxy_volume
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !proxy_volume
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err(PolicyError::InvalidProxyVolume);
        }
        Ok(Self {
            image,
            proxy_volume,
        })
    }
}

impl Default for GatewayRuntimeConfig {
    fn default() -> Self {
        Self::new(DEFAULT_GATEWAY_IMAGE, DEFAULT_PROXY_VOLUME)
            .expect("fixed Gateway defaults are valid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResourceNames {
    pub plugin_container: String,
    pub gateway_container: String,
    pub network: String,
}

impl RuntimeResourceNames {
    pub fn for_plugin(plugin_id: &PluginId) -> Self {
        let short_hash = short_plugin_hash(plugin_id);
        Self {
            plugin_container: format!("audiodown-plugin-{short_hash}"),
            gateway_container: format!("audiodown-gateway-{short_hash}"),
            network: format!("audiodown-plugin-network-{short_hash}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginNetworkSpec {
    pub name: String,
    pub labels: HashMap<String, String>,
    pub internal: bool,
    pub attachable: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimePluginContainerSpec {
    pub plugin_id: PluginId,
    pub image_id: String,
    pub container_name: String,
    pub command: Vec<String>,
    pub labels: HashMap<String, String>,
    pub read_only: bool,
    pub tmpfs: Vec<String>,
    pub mounts: Vec<String>,
    pub public_ports: Vec<u16>,
    pub privileged: bool,
    pub cap_add: Vec<String>,
    pub cap_drop: Vec<String>,
    pub security_opt: Vec<String>,
    pub pids_limit: i64,
    pub memory_bytes: i64,
    pub nano_cpus: i64,
    pub host_network: bool,
    pub network_name: String,
    pub proxy_url: &'static str,
    pub proxy_token: ProxyToken,
    pub dns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GatewayContainerSpec {
    pub image: String,
    pub container_name: String,
    pub labels: HashMap<String, String>,
    pub read_only: bool,
    pub tmpfs: Vec<String>,
    pub mounts: Vec<String>,
    pub public_ports: Vec<u16>,
    pub privileged: bool,
    pub cap_add: Vec<String>,
    pub cap_drop: Vec<String>,
    pub security_opt: Vec<String>,
    pub pids_limit: i64,
    pub memory_bytes: i64,
    pub nano_cpus: i64,
    pub host_network: bool,
    pub network_name: String,
    pub network_alias: &'static str,
    pub backend_socket: &'static str,
    pub proxy_volume: String,
    pub dns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PluginRuntimePolicy {
    pub plugin: RuntimePluginContainerSpec,
    pub gateway: GatewayContainerSpec,
    pub network: PluginNetworkSpec,
}

impl PluginRuntimePolicy {
    pub fn build(
        installed: InstalledPlugin,
        gateway_config: GatewayRuntimeConfig,
        proxy_token: ProxyToken,
    ) -> Result<Self, PolicyError> {
        let mut plugin = PluginContainerPolicy::build(installed)?;
        let names = RuntimeResourceNames::for_plugin(&plugin.plugin_id);
        plugin.container_name.clone_from(&names.plugin_container);
        plugin
            .labels
            .insert("io.audiodown.resource".to_string(), "plugin".to_string());
        let network_labels = managed_labels(&plugin.plugin_id, &plugin.labels, "network");
        let gateway_labels = managed_labels(&plugin.plugin_id, &plugin.labels, "gateway");

        Ok(Self {
            network: PluginNetworkSpec {
                name: names.network.clone(),
                labels: network_labels,
                internal: true,
                attachable: false,
            },
            gateway: GatewayContainerSpec {
                image: gateway_config.image,
                container_name: names.gateway_container,
                labels: gateway_labels,
                read_only: true,
                tmpfs: vec!["/tmp:rw,noexec,nosuid,nodev,size=16777216".to_string()],
                mounts: vec![format!(
                    "{}:{PROXY_MOUNT_PATH}:ro",
                    gateway_config.proxy_volume
                )],
                public_ports: Vec::new(),
                privileged: false,
                cap_add: Vec::new(),
                cap_drop: vec!["ALL".to_string()],
                security_opt: vec!["no-new-privileges:true".to_string()],
                pids_limit: GATEWAY_PIDS_LIMIT,
                memory_bytes: GATEWAY_MEMORY_BYTES,
                nano_cpus: GATEWAY_NANO_CPUS,
                host_network: false,
                network_name: names.network.clone(),
                network_alias: PROXY_GATEWAY_ALIAS,
                backend_socket: PROXY_BACKEND_SOCKET,
                proxy_volume: gateway_config.proxy_volume,
                dns: vec!["0.0.0.0".to_string()],
            },
            plugin: RuntimePluginContainerSpec {
                plugin_id: plugin.plugin_id,
                image_id: plugin.image_id,
                container_name: plugin.container_name,
                command: plugin.command,
                labels: plugin.labels,
                read_only: plugin.read_only,
                tmpfs: plugin.tmpfs,
                mounts: plugin.mounts,
                public_ports: plugin.public_ports,
                privileged: plugin.privileged,
                cap_add: plugin.cap_add,
                cap_drop: plugin.cap_drop,
                security_opt: plugin.security_opt,
                pids_limit: plugin.pids_limit,
                memory_bytes: plugin.memory_bytes,
                nano_cpus: plugin.nano_cpus,
                host_network: plugin.host_network,
                network_name: names.network,
                proxy_url: PROXY_GATEWAY_URL,
                proxy_token,
                dns: vec!["0.0.0.0".to_string()],
            },
        })
    }
}

pub struct PluginContainerPolicy;

impl PluginContainerPolicy {
    pub fn build(installed: InstalledPlugin) -> Result<PluginContainerSpec, PolicyError> {
        if installed.memory_bytes <= 0 || installed.nano_cpus <= 0 || installed.pids_limit <= 0 {
            return Err(PolicyError::InvalidLimit);
        }
        if !installed.runtime_path.starts_with("/plugin/") {
            return Err(PolicyError::InvalidRuntimePath);
        }

        let short_hash = short_plugin_hash(&installed.plugin_id);
        let container_name = format!("audiodown-plugin-{short_hash}");
        let mut labels = HashMap::new();
        labels.insert("io.audiodown.managed".to_string(), "true".to_string());
        labels.insert(
            "io.audiodown.installation".to_string(),
            installed.installation_id,
        );
        labels.insert(
            "io.audiodown.plugin-id".to_string(),
            installed.plugin_id.as_str().to_string(),
        );
        Ok(PluginContainerSpec {
            plugin_id: installed.plugin_id,
            image_id: installed.image_id,
            container_name,
            command: vec!["node".to_string(), installed.runtime_path],
            labels,
            read_only: true,
            tmpfs: vec!["/tmp:rw,noexec,nosuid,nodev,size=67108864".to_string()],
            mounts: Vec::new(),
            public_ports: Vec::new(),
            privileged: false,
            cap_add: Vec::new(),
            cap_drop: vec!["ALL".to_string()],
            security_opt: vec!["no-new-privileges:true".to_string()],
            pids_limit: installed.pids_limit,
            memory_bytes: installed.memory_bytes,
            nano_cpus: installed.nano_cpus,
            host_network: false,
        })
    }
}

fn short_plugin_hash(plugin_id: &PluginId) -> String {
    let digest = Sha256::digest(plugin_id.as_str().as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn managed_labels(
    plugin_id: &PluginId,
    plugin_labels: &HashMap<String, String>,
    resource: &str,
) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    labels.insert("io.audiodown.managed".to_string(), "true".to_string());
    labels.insert(
        "io.audiodown.installation".to_string(),
        plugin_labels["io.audiodown.installation"].clone(),
    );
    labels.insert(
        "io.audiodown.plugin-id".to_string(),
        plugin_id.as_str().to_string(),
    );
    labels.insert("io.audiodown.resource".to_string(), resource.to_string());
    labels
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("plugin resource limits must be positive")]
    InvalidLimit,
    #[error("plugin runtime path must remain under /plugin")]
    InvalidRuntimePath,
    #[error("fixed Gateway image configuration is invalid")]
    InvalidGatewayImage,
    #[error("proxy volume configuration is invalid")]
    InvalidProxyVolume,
}
