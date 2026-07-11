use std::collections::HashMap;

use audiodown_domain::plugin::PluginId;
use sha2::{Digest, Sha256};
use thiserror::Error;

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

pub struct PluginContainerPolicy;

impl PluginContainerPolicy {
    pub fn build(installed: InstalledPlugin) -> Result<PluginContainerSpec, PolicyError> {
        if installed.memory_bytes <= 0 || installed.nano_cpus <= 0 || installed.pids_limit <= 0 {
            return Err(PolicyError::InvalidLimit);
        }
        if !installed.runtime_path.starts_with("/plugin/") {
            return Err(PolicyError::InvalidRuntimePath);
        }

        let digest = Sha256::digest(installed.plugin_id.as_str().as_bytes());
        let short_hash = digest[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
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
        let rpc_mount = format!(
            "/run/audiodown/plugins/{}:/run/audiodown:rw",
            installed.plugin_id
        );

        Ok(PluginContainerSpec {
            plugin_id: installed.plugin_id,
            image_id: installed.image_id,
            container_name,
            command: vec!["node".to_string(), installed.runtime_path],
            labels,
            read_only: true,
            tmpfs: vec!["/tmp:rw,noexec,nosuid,nodev,size=67108864".to_string()],
            mounts: vec![rpc_mount],
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

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("plugin resource limits must be positive")]
    InvalidLimit,
    #[error("plugin runtime path must remain under /plugin")]
    InvalidRuntimePath,
}
