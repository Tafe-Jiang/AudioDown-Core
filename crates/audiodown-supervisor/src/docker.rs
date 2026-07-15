use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::{
    content::ContentMethod,
    rpc::{JsonRpcRequest, JsonRpcResponse},
};
use audiodown_supervisor_protocol::{PluginRpcResult, ProtocolError, ProxyToken};
use bollard::{
    container::LogOutput,
    exec::{CreateExecOptions, StartExecResults},
    models::{
        ContainerCreateBody, EndpointSettings, HostConfig, NetworkCreateRequest, NetworkingConfig,
    },
    query_parameters::{
        CreateContainerOptionsBuilder, ListNetworksOptionsBuilder, LogsOptionsBuilder,
        RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
    },
    Docker,
};
use futures_util::StreamExt;
use serde::Serialize;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{
    install_record::ValidatedInstall,
    policy::{GatewayRuntimeConfig, PluginRuntimePolicy, PolicyError, RuntimeResourceNames},
};

const RPC_SOCKET: &str = "/tmp/audiodown-rpc.sock";
const HANDSHAKE_ATTEMPTS: usize = 40;
const HANDSHAKE_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_PLUGIN_RPC_BYTES: usize = 1024 * 1024;
pub const PLUGIN_RPC_TIMEOUT: Duration = Duration::from_secs(8);
pub const PROXY_TOKEN_SECRET_DIR: &str = "/run/audiodown-secrets";
const PROXY_TOKEN_SECRET_PATH: &str = "/run/audiodown-secrets/proxy-token";
const PROXY_TOKEN_TEMPORARY_PATH: &str = "/run/audiodown-secrets/.proxy-token.tmp";
const PLUGIN_BOOTSTRAP_SCRIPT: &str = r#"set -eu
secret_file=/run/audiodown-secrets/proxy-token
attempt=0
while [ ! -f "$secret_file" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 200 ]; then
    echo 'Plugin proxy token was not delivered' >&2
    exit 1
  fi
  sleep 0.05
done
AUDIODOWN_PROXY_TOKEN="$(cat "$secret_file")"
rm -f "$secret_file"
[ -n "$AUDIODOWN_PROXY_TOKEN" ] || exit 1
export AUDIODOWN_PROXY_TOKEN
unset secret_file attempt
exec "$@""#;

pub struct DockerAdapter {
    docker: Docker,
    installation_id: String,
    gateway_config: GatewayRuntimeConfig,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginLog {
    pub level: String,
    pub message: String,
    pub context: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct StartedPlugin {
    pub container_id: String,
    pub logs: Vec<PluginLog>,
}

#[derive(Debug, Clone)]
pub struct ManagedRemovalPlan {
    pub plugin_id: PluginId,
    pub image_id: String,
    pub install_directory: PathBuf,
    pub expected_image_labels: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ContentRpcExecPlan {
    pub command: Vec<String>,
    pub user: String,
    pub working_dir: String,
}

#[derive(Debug, Default)]
struct RuntimeCleanup {
    plugin_container_id: Option<String>,
}

impl DockerAdapter {
    pub fn connect(installation_id: String) -> Result<Self, DockerAdapterError> {
        Ok(Self {
            docker: Docker::connect_with_local_defaults()?,
            installation_id,
            gateway_config: GatewayRuntimeConfig::default(),
        })
    }

    pub fn configure_gateway(&mut self, gateway_config: GatewayRuntimeConfig) {
        self.gateway_config = gateway_config;
    }

    pub async fn reconcile_runtime_resources(&self) -> Result<(), DockerAdapterError> {
        let mut plugin_ids = HashSet::new();
        let mut discovery_failures = 0;
        for resource in ["plugin", "gateway"] {
            let mut filters = HashMap::new();
            filters.insert(
                "label".to_string(),
                vec![
                    "io.audiodown.managed=true".to_string(),
                    format!("io.audiodown.installation={}", self.installation_id),
                    format!("io.audiodown.resource={resource}"),
                ],
            );
            match self
                .docker
                .list_containers(Some(
                    bollard::query_parameters::ListContainersOptionsBuilder::new()
                        .all(true)
                        .filters(&filters)
                        .build(),
                ))
                .await
            {
                Ok(containers) => {
                    for container in containers {
                        let labels = container.labels.unwrap_or_default();
                        match runtime_plugin_id(&labels, &self.installation_id, resource) {
                            Ok(plugin_id) => {
                                plugin_ids.insert(plugin_id);
                            }
                            Err(_) => discovery_failures += 1,
                        }
                    }
                }
                Err(_) => discovery_failures += 1,
            }
        }

        let mut network_filters = HashMap::new();
        network_filters.insert(
            "label".to_string(),
            vec![
                "io.audiodown.managed=true".to_string(),
                format!("io.audiodown.installation={}", self.installation_id),
                "io.audiodown.resource=network".to_string(),
            ],
        );
        match self
            .docker
            .list_networks(Some(
                ListNetworksOptionsBuilder::new()
                    .filters(&network_filters)
                    .build(),
            ))
            .await
        {
            Ok(networks) => {
                for network in networks {
                    let labels = network.labels.unwrap_or_default();
                    match runtime_plugin_id(&labels, &self.installation_id, "network") {
                        Ok(plugin_id) => {
                            plugin_ids.insert(plugin_id);
                        }
                        Err(_) => discovery_failures += 1,
                    }
                }
            }
            Err(_) => discovery_failures += 1,
        }

        let mut results = Vec::with_capacity(plugin_ids.len() + discovery_failures);
        results.extend((0..discovery_failures).map(|_| Err(())));
        for plugin_id in plugin_ids {
            results.push(
                self.cleanup_runtime_resources(&plugin_id)
                    .await
                    .map(|_| ())
                    .map_err(|_| ()),
            );
        }
        reconcile_cleanup_results(results)
    }

    pub async fn start_plugin(
        &self,
        install: ValidatedInstall,
        proxy_token: ProxyToken,
    ) -> Result<StartedPlugin, DockerAdapterError> {
        let plugin_id = install.installed.plugin_id.clone();
        let plugin_version = install.manifest.version.to_string();
        self.cleanup_runtime_resources(&plugin_id).await?;
        self.verify_install_image(&install).await?;
        let policy = PluginRuntimePolicy::build(
            install.installed,
            self.gateway_config.clone(),
            proxy_token,
        )?;

        let started = self.start_runtime(&policy, &plugin_version).await;
        match started {
            Ok(started) => Ok(started),
            Err(start_error) => match self.cleanup_runtime_resources(&plugin_id).await {
                Ok(_) => Err(start_error),
                Err(_) => Err(DockerAdapterError::RuntimeCleanupFailed),
            },
        }
    }

    pub async fn stop_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<String>, DockerAdapterError> {
        Ok(self
            .cleanup_runtime_resources(plugin_id)
            .await?
            .plugin_container_id)
    }

    pub async fn inspect_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginInspection, DockerAdapterError> {
        let Some(container_id) = self.find_managed_container(plugin_id).await? else {
            return Ok(PluginInspection {
                container_id: None,
                running: false,
            });
        };
        let Some(gateway_id) = self
            .find_managed_container_by_resource(plugin_id, "gateway")
            .await?
        else {
            return Ok(PluginInspection {
                container_id: Some(container_id),
                running: false,
            });
        };
        let plugin_inspect = self
            .docker
            .inspect_container(
                &container_id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await?;
        let gateway_inspect = self
            .docker
            .inspect_container(
                &gateway_id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await?;
        let names = RuntimeResourceNames::for_plugin(plugin_id);
        let network_running = self
            .inspect_managed_network(plugin_id, &names.network)
            .await?;
        Ok(PluginInspection {
            container_id: Some(container_id),
            running: network_running
                && plugin_inspect
                    .state
                    .and_then(|state| state.running)
                    .unwrap_or(false)
                && gateway_inspect
                    .state
                    .and_then(|state| state.running)
                    .unwrap_or(false),
        })
    }

    pub async fn plugin_logs(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Vec<PluginLog>, DockerAdapterError> {
        let Some(container_id) = self.find_managed_container(plugin_id).await? else {
            return Ok(Vec::new());
        };
        self.plugin_logs_by_container(&container_id).await
    }

    pub async fn invoke_plugin(
        &self,
        plugin_id: &PluginId,
        method: ContentMethod,
        params: serde_json::Value,
    ) -> Result<PluginRpcResult, DockerAdapterError> {
        let container_id = self
            .find_managed_container(plugin_id)
            .await?
            .ok_or(DockerAdapterError::PluginNotRunning)?;
        let inspection = self
            .docker
            .inspect_container(
                &container_id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await?;
        if !inspection
            .state
            .and_then(|state| state.running)
            .unwrap_or(false)
        {
            return Err(DockerAdapterError::PluginNotRunning);
        }

        let request_id = Uuid::new_v4().to_string();
        let plan = build_content_rpc_exec(plugin_id, method, params, &request_id)?;
        let execution = async {
            let exec = self
                .docker
                .create_exec(
                    &container_id,
                    CreateExecOptions {
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        cmd: Some(plan.command.iter().map(String::as_str).collect()),
                        user: Some(&plan.user),
                        working_dir: Some(&plan.working_dir),
                        ..Default::default()
                    },
                )
                .await?;
            let mut output = match self.docker.start_exec(&exec.id, None).await? {
                StartExecResults::Attached { output, .. } => output,
                StartExecResults::Detached => return Err(DockerAdapterError::RpcExecFailed),
            };
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            while let Some(chunk) = output.next().await {
                match chunk? {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => {
                        append_rpc_bytes(&mut stdout, &stderr, &message)?;
                    }
                    LogOutput::StdErr { message } => {
                        append_rpc_bytes(&mut stderr, &stdout, &message)?;
                    }
                    LogOutput::StdIn { .. } => {}
                }
            }
            let inspection = self.docker.inspect_exec(&exec.id).await?;
            let exit_code = inspection
                .exit_code
                .ok_or(DockerAdapterError::RpcExecFailed)?;
            parse_content_rpc_output(&request_id, &stdout, &stderr, exit_code)
        };

        tokio::time::timeout(PLUGIN_RPC_TIMEOUT, execution)
            .await
            .map_err(|_| DockerAdapterError::RpcTimeout)?
    }

    pub async fn remove_plugin(
        &self,
        plugin_data: &Path,
        install: ValidatedInstall,
    ) -> Result<audiodown_supervisor_protocol::PluginRemoveResult, DockerAdapterError> {
        let plan = managed_removal_plan(plugin_data, &self.installation_id, &install)?;
        let cleanup = self.cleanup_runtime_resources(&plan.plugin_id).await?;
        let image = self.docker.inspect_image(&plan.image_id).await?;
        let labels = image
            .config
            .and_then(|config| config.labels)
            .unwrap_or_default();
        if !plan
            .expected_image_labels
            .iter()
            .all(|(key, value)| labels.get(key) == Some(value))
        {
            return Err(DockerAdapterError::ImageLabelMismatch);
        }

        self.docker
            .remove_image(
                &plan.image_id,
                Some(
                    RemoveImageOptionsBuilder::new()
                        .force(true)
                        .noprune(false)
                        .build(),
                ),
                None,
            )
            .await?;
        remove_managed_install_directory(&plan.install_directory)?;

        Ok(audiodown_supervisor_protocol::PluginRemoveResult {
            plugin_id: plan.plugin_id,
            removed_container: cleanup.plugin_container_id.is_some(),
            removed_image: true,
            removed_install_directory: true,
        })
    }

    pub async fn find_managed_container(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<String>, DockerAdapterError> {
        self.find_managed_container_by_resource(plugin_id, "plugin")
            .await
    }

    async fn find_managed_container_by_resource(
        &self,
        plugin_id: &PluginId,
        resource: &str,
    ) -> Result<Option<String>, DockerAdapterError> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![
                "io.audiodown.managed=true".to_string(),
                format!("io.audiodown.installation={}", self.installation_id),
                format!("io.audiodown.plugin-id={plugin_id}"),
                format!("io.audiodown.resource={resource}"),
            ],
        );
        let containers = self
            .docker
            .list_containers(Some(
                bollard::query_parameters::ListContainersOptionsBuilder::new()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await?;

        let mut found = None;
        for container in containers {
            let labels = container.labels.unwrap_or_default();
            if verify_managed_labels(&labels, &self.installation_id, resource).is_err()
                || labels.get("io.audiodown.plugin-id").map(String::as_str)
                    != Some(plugin_id.as_str())
            {
                return Err(DockerAdapterError::LabelMismatch);
            }
            if let Some(id) = container.id {
                if found.replace(id).is_some() {
                    return Err(DockerAdapterError::LabelMismatch);
                }
            }
        }
        Ok(found)
    }

    async fn start_runtime(
        &self,
        policy: &PluginRuntimePolicy,
        plugin_version: &str,
    ) -> Result<StartedPlugin, DockerAdapterError> {
        self.docker
            .create_network(NetworkCreateRequest {
                name: policy.network.name.clone(),
                driver: Some("bridge".to_string()),
                internal: Some(policy.network.internal),
                attachable: Some(policy.network.attachable),
                ingress: Some(false),
                enable_ipv6: Some(false),
                options: Some(HashMap::from([(
                    "com.docker.network.bridge.enable_ip_masquerade".to_string(),
                    "false".to_string(),
                )])),
                labels: Some(policy.network.labels.clone()),
                ..Default::default()
            })
            .await?;

        let gateway_tmpfs = tmpfs_options(&policy.gateway.tmpfs);
        let gateway_config = ContainerCreateBody {
            image: Some(policy.gateway.image.clone()),
            user: Some("10003:10003".to_string()),
            working_dir: Some("/".to_string()),
            env: Some(Vec::new()),
            labels: Some(policy.gateway.labels.clone()),
            network_disabled: Some(false),
            attach_stdout: Some(false),
            attach_stderr: Some(false),
            open_stdin: Some(false),
            tty: Some(false),
            host_config: Some(HostConfig {
                memory: Some(policy.gateway.memory_bytes),
                memory_swap: Some(policy.gateway.memory_bytes),
                nano_cpus: Some(policy.gateway.nano_cpus),
                pids_limit: Some(policy.gateway.pids_limit),
                binds: Some(policy.gateway.mounts.clone()),
                network_mode: Some(policy.gateway.network_name.clone()),
                privileged: Some(policy.gateway.privileged),
                publish_all_ports: Some(false),
                readonly_rootfs: Some(policy.gateway.read_only),
                cap_add: Some(policy.gateway.cap_add.clone()),
                cap_drop: Some(policy.gateway.cap_drop.clone()),
                security_opt: Some(policy.gateway.security_opt.clone()),
                tmpfs: Some(gateway_tmpfs),
                dns: Some(policy.gateway.dns.clone()),
                init: Some(true),
                auto_remove: Some(false),
                ..Default::default()
            }),
            networking_config: Some(networking_config(
                &policy.gateway.network_name,
                vec![policy.gateway.network_alias.to_string()],
            )),
            ..Default::default()
        };
        let gateway = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&policy.gateway.container_name)
                        .build(),
                ),
                gateway_config,
            )
            .await?;
        self.docker
            .start_container(
                &gateway.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await?;

        let plugin_config = plugin_container_config(&policy.plugin);
        let plugin = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&policy.plugin.container_name)
                        .build(),
                ),
                plugin_config,
            )
            .await?;
        self.docker
            .start_container(
                &plugin.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await?;
        self.deliver_proxy_token(&plugin.id, &policy.plugin.proxy_token)
            .await?;
        self.wait_for_handshake(&plugin.id, &policy.plugin.plugin_id, plugin_version)
            .await?;
        let logs = self.plugin_logs_by_container(&plugin.id).await?;
        Ok(StartedPlugin {
            container_id: plugin.id,
            logs,
        })
    }

    async fn cleanup_runtime_resources(
        &self,
        plugin_id: &PluginId,
    ) -> Result<RuntimeCleanup, DockerAdapterError> {
        let names = RuntimeResourceNames::for_plugin(plugin_id);
        let mut cleanup = RuntimeCleanup::default();
        let mut failed = false;

        match self.find_managed_container(plugin_id).await {
            Ok(Some(container_id)) => {
                cleanup.plugin_container_id = Some(container_id.clone());
                if self.remove_runtime_container(&container_id).await.is_err() {
                    failed = true;
                }
            }
            Ok(None) => {}
            Err(_) => failed = true,
        }
        match self
            .find_managed_container_by_resource(plugin_id, "gateway")
            .await
        {
            Ok(Some(container_id)) => {
                if self.remove_runtime_container(&container_id).await.is_err() {
                    failed = true;
                }
            }
            Ok(None) => {}
            Err(_) => failed = true,
        }
        match self.remove_managed_network(plugin_id, &names.network).await {
            Ok(()) => {}
            Err(_) => failed = true,
        }
        if failed {
            Err(DockerAdapterError::RuntimeCleanupFailed)
        } else {
            Ok(cleanup)
        }
    }

    async fn remove_runtime_container(&self, container_id: &str) -> Result<(), DockerAdapterError> {
        match self
            .docker
            .remove_container(
                container_id,
                Some(
                    RemoveContainerOptionsBuilder::new()
                        .force(true)
                        .v(true)
                        .build(),
                ),
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(error) if is_not_found(&error) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn inspect_managed_network(
        &self,
        plugin_id: &PluginId,
        network_name: &str,
    ) -> Result<bool, DockerAdapterError> {
        let network = match self
            .docker
            .inspect_network(
                network_name,
                None::<bollard::query_parameters::InspectNetworkOptions>,
            )
            .await
        {
            Ok(network) => network,
            Err(error) if is_not_found(&error) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let labels = network.labels.unwrap_or_default();
        verify_runtime_labels(&labels, &self.installation_id, plugin_id, "network")?;
        Ok(network_is_healthy(
            network.internal == Some(true),
            network.attachable == Some(true),
        ))
    }

    async fn remove_managed_network(
        &self,
        plugin_id: &PluginId,
        network_name: &str,
    ) -> Result<(), DockerAdapterError> {
        let network = match self
            .docker
            .inspect_network(
                network_name,
                None::<bollard::query_parameters::InspectNetworkOptions>,
            )
            .await
        {
            Ok(network) => network,
            Err(error) if is_not_found(&error) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if !network_is_owned_for_cleanup(
            &network.labels.unwrap_or_default(),
            &self.installation_id,
            plugin_id,
        ) {
            return Err(DockerAdapterError::LabelMismatch);
        }
        match self.docker.remove_network(network_name).await {
            Ok(()) => Ok(()),
            Err(error) if is_not_found(&error) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn verify_install_image(
        &self,
        install: &ValidatedInstall,
    ) -> Result<(), DockerAdapterError> {
        let Some(expected) = install.expected_image_labels.as_ref() else {
            return Ok(());
        };
        let image = self
            .docker
            .inspect_image(&install.installed.image_id)
            .await?;
        let labels = image
            .config
            .and_then(|config| config.labels)
            .unwrap_or_default();
        if expected
            .iter()
            .all(|(key, value)| labels.get(key) == Some(value))
        {
            Ok(())
        } else {
            Err(DockerAdapterError::ImageLabelMismatch)
        }
    }

    async fn wait_for_handshake(
        &self,
        container_id: &str,
        plugin_id: &PluginId,
        plugin_version: &str,
    ) -> Result<(), DockerAdapterError> {
        let mut last_error = None;
        for _ in 0..HANDSHAKE_ATTEMPTS {
            match self
                .handshake(container_id, plugin_id, plugin_version)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
            tokio::time::sleep(HANDSHAKE_RETRY_DELAY).await;
        }
        Err(DockerAdapterError::Handshake(last_error.unwrap_or_else(
            || "plugin did not expose its RPC socket".to_string(),
        )))
    }

    async fn deliver_proxy_token(
        &self,
        container_id: &str,
        proxy_token: &ProxyToken,
    ) -> Result<(), DockerAdapterError> {
        let token = proxy_token.expose_secret(|value| value.as_bytes().to_vec());
        let command = proxy_token_publish_command(
            token.len(),
            Path::new(PROXY_TOKEN_TEMPORARY_PATH),
            Path::new(PROXY_TOKEN_SECRET_PATH),
        );
        let exec = self
            .docker
            .create_exec(
                container_id,
                CreateExecOptions {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(command),
                    user: Some("10002:10002".to_string()),
                    working_dir: Some(PROXY_TOKEN_SECRET_DIR.to_string()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| DockerAdapterError::SecretDeliveryFailed)?;
        let StartExecResults::Attached { output, mut input } = self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|_| DockerAdapterError::SecretDeliveryFailed)?
        else {
            return Err(DockerAdapterError::SecretDeliveryFailed);
        };
        input
            .write_all(&token)
            .await
            .map_err(|_| DockerAdapterError::SecretDeliveryFailed)?;
        drop(input);
        drop(output);
        for _ in 0..100 {
            let inspection = self
                .docker
                .inspect_exec(&exec.id)
                .await
                .map_err(|_| DockerAdapterError::SecretDeliveryFailed)?;
            if inspection.running == Some(false) {
                return if inspection.exit_code == Some(0) {
                    Ok(())
                } else {
                    Err(DockerAdapterError::SecretDeliveryFailed)
                };
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err(DockerAdapterError::SecretDeliveryFailed)
    }

    async fn handshake(
        &self,
        container_id: &str,
        plugin_id: &PluginId,
        plugin_version: &str,
    ) -> Result<(), String> {
        let exec = self
            .docker
            .create_exec(
                container_id,
                CreateExecOptions {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(vec![
                        "node",
                        "-e",
                        HANDSHAKE_SCRIPT,
                        RPC_SOCKET,
                        plugin_id.as_str(),
                        plugin_version,
                    ]),
                    user: Some("10002:10002"),
                    working_dir: Some("/tmp"),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        let mut output = match self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|error| error.to_string())?
        {
            StartExecResults::Attached { output, .. } => output,
            StartExecResults::Detached => {
                return Err("handshake exec unexpectedly detached".to_string())
            }
        };
        let mut bytes = Vec::new();
        while let Some(chunk) = output.next().await {
            let chunk = chunk.map_err(|error| error.to_string())?;
            bytes.extend_from_slice(chunk.as_ref());
        }
        let inspection = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|error| error.to_string())?;
        if inspection.exit_code != Some(0) {
            return Err(String::from_utf8_lossy(&bytes).trim().to_string());
        }
        Ok(())
    }

    async fn plugin_logs_by_container(
        &self,
        container_id: &str,
    ) -> Result<Vec<PluginLog>, DockerAdapterError> {
        let options = LogsOptionsBuilder::new()
            .follow(false)
            .stdout(true)
            .stderr(true)
            .timestamps(false)
            .tail("100")
            .build();
        let mut stream = self.docker.logs(container_id, Some(options));
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(chunk?.as_ref());
        }

        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(parse_plugin_log)
            .collect())
    }
}

#[doc(hidden)]
pub fn proxy_token_publish_command(
    token_len: usize,
    temporary_path: &Path,
    final_path: &Path,
) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        r#"set -eu
expected=$1
temporary=$2
final=$3
cleanup() { rm -f "$temporary"; }
trap cleanup 0
trap 'exit 1' 1 2 15
umask 077
rm -f "$temporary" "$final"
head -c "$expected" > "$temporary"
actual=$(wc -c < "$temporary")
[ "$actual" -eq "$expected" ]
chmod 600 "$temporary"
mv "$temporary" "$final"
trap - 0 1 2 15"#
            .to_string(),
        "audiodown-secret-publish".to_string(),
        token_len.to_string(),
        temporary_path.to_string_lossy().into_owned(),
        final_path.to_string_lossy().into_owned(),
    ]
}

pub fn plugin_container_config(
    policy: &crate::policy::RuntimePluginContainerSpec,
) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(policy.image_id.clone()),
        user: Some("10002:10002".to_string()),
        working_dir: Some("/plugin".to_string()),
        entrypoint: Some(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            PLUGIN_BOOTSTRAP_SCRIPT.to_string(),
            "audiodown-plugin-bootstrap".to_string(),
        ]),
        cmd: Some(policy.command.clone()),
        env: Some(vec![
            format!("AUDIODOWN_RPC_SOCKET={RPC_SOCKET}"),
            "AUDIODOWN_NODE_SDK_PATH=/sdk/src/index.js".to_string(),
            format!("AUDIODOWN_PROXY_URL={}", policy.proxy_url),
            "NODE_ENV=production".to_string(),
        ]),
        labels: Some(policy.labels.clone()),
        network_disabled: Some(false),
        attach_stdout: Some(false),
        attach_stderr: Some(false),
        open_stdin: Some(false),
        tty: Some(false),
        host_config: Some(HostConfig {
            memory: Some(policy.memory_bytes),
            memory_swap: Some(policy.memory_bytes),
            nano_cpus: Some(policy.nano_cpus),
            pids_limit: Some(policy.pids_limit),
            binds: Some(policy.mounts.clone()),
            network_mode: Some(policy.network_name.clone()),
            privileged: Some(policy.privileged),
            publish_all_ports: Some(false),
            readonly_rootfs: Some(policy.read_only),
            cap_add: Some(policy.cap_add.clone()),
            cap_drop: Some(policy.cap_drop.clone()),
            security_opt: Some(policy.security_opt.clone()),
            tmpfs: Some(tmpfs_options(&policy.tmpfs)),
            dns: Some(policy.dns.clone()),
            init: Some(true),
            auto_remove: Some(false),
            ..Default::default()
        }),
        networking_config: Some(networking_config(
            &policy.network_name,
            vec![policy.container_name.clone()],
        )),
        ..Default::default()
    }
}

pub fn network_is_owned_for_cleanup(
    labels: &HashMap<String, String>,
    installation_id: &str,
    plugin_id: &PluginId,
) -> bool {
    verify_runtime_labels(labels, installation_id, plugin_id, "network").is_ok()
}

pub fn network_is_healthy(internal: bool, attachable: bool) -> bool {
    internal && !attachable
}

pub fn discover_runtime_plugin_ids<'a>(
    installation_id: &str,
    resources: impl IntoIterator<Item = &'a HashMap<String, String>>,
) -> Result<HashSet<PluginId>, DockerAdapterError> {
    resources
        .into_iter()
        .map(|labels| {
            let resource = labels
                .get("io.audiodown.resource")
                .map(String::as_str)
                .ok_or(DockerAdapterError::LabelMismatch)?;
            if !matches!(resource, "plugin" | "gateway" | "network") {
                return Err(DockerAdapterError::LabelMismatch);
            }
            runtime_plugin_id(labels, installation_id, resource)
        })
        .collect()
}

pub fn reconcile_cleanup_results<I, E>(results: I) -> Result<(), DockerAdapterError>
where
    I: IntoIterator<Item = Result<(), E>>,
{
    let failures = results.into_iter().filter(Result::is_err).count();
    if failures == 0 {
        Ok(())
    } else {
        Err(DockerAdapterError::RuntimeReconcileFailed(failures))
    }
}

fn runtime_plugin_id(
    labels: &HashMap<String, String>,
    installation_id: &str,
    resource: &str,
) -> Result<PluginId, DockerAdapterError> {
    verify_managed_labels(labels, installation_id, resource)?;
    labels
        .get("io.audiodown.plugin-id")
        .ok_or(DockerAdapterError::LabelMismatch)
        .and_then(|value| PluginId::parse(value).map_err(|_| DockerAdapterError::LabelMismatch))
}

fn tmpfs_options(entries: &[String]) -> HashMap<String, String> {
    entries
        .iter()
        .filter_map(|entry| entry.split_once(':'))
        .map(|(path, options)| (path.to_string(), options.to_string()))
        .collect()
}

fn networking_config(network_name: &str, aliases: Vec<String>) -> NetworkingConfig {
    NetworkingConfig {
        endpoints_config: Some(HashMap::from([(
            network_name.to_string(),
            EndpointSettings {
                aliases: Some(aliases),
                ..Default::default()
            },
        )])),
    }
}

fn verify_managed_labels(
    labels: &HashMap<String, String>,
    installation_id: &str,
    resource: &str,
) -> Result<(), DockerAdapterError> {
    if labels.get("io.audiodown.managed").map(String::as_str) != Some("true")
        || labels.get("io.audiodown.installation").map(String::as_str) != Some(installation_id)
        || labels.get("io.audiodown.resource").map(String::as_str) != Some(resource)
    {
        return Err(DockerAdapterError::LabelMismatch);
    }
    Ok(())
}

fn verify_runtime_labels(
    labels: &HashMap<String, String>,
    installation_id: &str,
    plugin_id: &PluginId,
    resource: &str,
) -> Result<(), DockerAdapterError> {
    verify_managed_labels(labels, installation_id, resource)?;
    if labels.get("io.audiodown.plugin-id").map(String::as_str) != Some(plugin_id.as_str()) {
        return Err(DockerAdapterError::LabelMismatch);
    }
    Ok(())
}

fn is_not_found(error: &bollard::errors::Error) -> bool {
    matches!(
        error,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

pub fn build_content_rpc_exec(
    _plugin_id: &PluginId,
    method: ContentMethod,
    params: serde_json::Value,
    request_id: &str,
) -> Result<ContentRpcExecPlan, DockerAdapterError> {
    if request_id.is_empty() || request_id.len() > 128 || request_id.contains('\0') {
        return Err(DockerAdapterError::InvalidRpcRequest);
    }
    let request = JsonRpcRequest::new(request_id, method.capability(), params)
        .map_err(|_| DockerAdapterError::InvalidRpcRequest)?;
    let request =
        serde_json::to_string(&request).map_err(|_| DockerAdapterError::InvalidRpcRequest)?;
    if request.len() > MAX_PLUGIN_RPC_BYTES {
        return Err(DockerAdapterError::InvalidRpcRequest);
    }
    Ok(ContentRpcExecPlan {
        command: vec![
            "node".to_string(),
            "-e".to_string(),
            CONTENT_RPC_SCRIPT.to_string(),
            RPC_SOCKET.to_string(),
            request,
            request_id.to_string(),
        ],
        user: "10002:10002".to_string(),
        working_dir: "/tmp".to_string(),
    })
}

pub fn parse_content_rpc_output(
    request_id: &str,
    stdout: &[u8],
    stderr: &[u8],
    exit_code: i64,
) -> Result<PluginRpcResult, DockerAdapterError> {
    if stdout.len().saturating_add(stderr.len()) > MAX_PLUGIN_RPC_BYTES {
        return Err(DockerAdapterError::RpcResponseTooLarge);
    }
    if exit_code != 0 {
        return Err(DockerAdapterError::RpcExecFailed);
    }
    if !stderr.is_empty() {
        return Err(DockerAdapterError::RpcStderr);
    }
    let output = std::str::from_utf8(stdout).map_err(|_| DockerAdapterError::InvalidRpcResponse)?;
    let lines = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.len() != 1 {
        return Err(DockerAdapterError::InvalidRpcResponse);
    }
    let response: JsonRpcResponse =
        serde_json::from_str(lines[0]).map_err(|_| DockerAdapterError::InvalidRpcResponse)?;
    if response.id != request_id {
        return Err(DockerAdapterError::InvalidRpcResponse);
    }
    PluginRpcResult::new(response).map_err(|error| match error {
        ProtocolError::RpcResponseTooLarge => DockerAdapterError::RpcResponseTooLarge,
        _ => DockerAdapterError::InvalidRpcResponse,
    })
}

fn append_rpc_bytes(
    destination: &mut Vec<u8>,
    other: &[u8],
    bytes: &[u8],
) -> Result<(), DockerAdapterError> {
    if destination
        .len()
        .saturating_add(other.len())
        .saturating_add(bytes.len())
        > MAX_PLUGIN_RPC_BYTES
    {
        return Err(DockerAdapterError::RpcResponseTooLarge);
    }
    destination.extend_from_slice(bytes);
    Ok(())
}

pub fn managed_removal_plan(
    plugin_data: &Path,
    installation_id: &str,
    install: &ValidatedInstall,
) -> Result<ManagedRemovalPlan, DockerAdapterError> {
    let expected = install
        .expected_image_labels
        .clone()
        .ok_or(DockerAdapterError::RemovalAttestationMismatch)?;
    if install.installed.installation_id != installation_id
        || expected.get("io.audiodown.managed").map(String::as_str) != Some("true")
        || expected
            .get("io.audiodown.installation")
            .map(String::as_str)
            != Some(installation_id)
        || expected.get("io.audiodown.plugin-id").map(String::as_str)
            != Some(install.installed.plugin_id.as_str())
    {
        return Err(DockerAdapterError::RemovalAttestationMismatch);
    }

    let install_directory = plugin_data
        .join("installed")
        .join(install.installed.plugin_id.as_str());
    validate_install_directory(&install_directory)?;
    Ok(ManagedRemovalPlan {
        plugin_id: install.installed.plugin_id.clone(),
        image_id: install.installed.image_id.clone(),
        install_directory,
        expected_image_labels: expected,
    })
}

fn validate_install_directory(path: &Path) -> Result<(), DockerAdapterError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| DockerAdapterError::UnsafeInstallDirectory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DockerAdapterError::UnsafeInstallDirectory);
    }
    Ok(())
}

fn remove_managed_install_directory(path: &Path) -> Result<(), DockerAdapterError> {
    validate_install_directory(path)?;
    std::fs::remove_dir_all(path).map_err(|_| DockerAdapterError::UnsafeInstallDirectory)
}

#[derive(Debug, Clone)]
pub struct PluginInspection {
    pub container_id: Option<String>,
    pub running: bool,
}

fn parse_plugin_log(line: &str) -> Option<PluginLog> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            return Some(PluginLog {
                level: "info".to_string(),
                message: line.to_string(),
                context: serde_json::json!({"stream": "stdio"}),
            })
        }
    };
    if value.get("method").and_then(serde_json::Value::as_str) != Some("log.emit") {
        return None;
    }
    let params = value.get("params")?;
    Some(PluginLog {
        level: params
            .get("level")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("info")
            .to_string(),
        message: params
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("plugin log event")
            .to_string(),
        context: params
            .get("context")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    })
}

const HANDSHAKE_SCRIPT: &str = r#"
const net = require("node:net");
const [socketPath, pluginId, pluginVersion] = process.argv.slice(1);
const socket = net.createConnection(socketPath);
let buffer = "";
const responses = new Map();
const fail = (message) => {
  process.stderr.write(`${message}\n`);
  socket.destroy();
  process.exit(1);
};
const timer = setTimeout(() => fail("RPC handshake timed out"), 1000);
socket.on("error", (error) => fail(error.message));
socket.on("connect", () => {
  socket.write(`${JSON.stringify({jsonrpc:"2.0",id:"hello",method:"system.hello",params:{}})}\n`);
  socket.write(`${JSON.stringify({jsonrpc:"2.0",id:"health",method:"system.health",params:{}})}\n`);
});
socket.on("data", (chunk) => {
  buffer += chunk.toString("utf8");
  let newline;
  while ((newline = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, newline);
    buffer = buffer.slice(newline + 1);
    if (!line) continue;
    const response = JSON.parse(line);
    responses.set(response.id, response);
  }
  if (!responses.has("hello") || !responses.has("health")) return;
  const hello = responses.get("hello").result;
  const health = responses.get("health").result;
  if (!hello || !health ||
      hello.pluginId !== pluginId ||
      hello.pluginVersion !== pluginVersion ||
      hello.protocolVersion !== "1.0" ||
      health.pluginId !== pluginId ||
      health.pluginVersion !== pluginVersion ||
      health.protocolVersion !== "1.0" ||
      health.healthy !== true) {
    fail("RPC handshake identity or protocol mismatch");
  }
  clearTimeout(timer);
  socket.end();
});
socket.on("end", () => process.exit(0));
"#;

const CONTENT_RPC_SCRIPT: &str = r#"
const net = require("node:net");
const [socketPath, requestJson, expectedId] = process.argv.slice(1);
const socket = net.createConnection(socketPath);
let buffer = "";
let complete = false;
const fail = (message) => {
  process.stderr.write(`${message}\n`);
  socket.destroy();
  process.exit(1);
};
const timer = setTimeout(() => fail("RPC call timed out"), 7500);
socket.on("error", (error) => fail(error.message));
socket.on("connect", () => socket.write(`${requestJson}\n`));
socket.on("data", (chunk) => {
  if (complete) return fail("RPC returned more than one response");
  buffer += chunk.toString("utf8");
  if (Buffer.byteLength(buffer, "utf8") > 1024 * 1024) {
    return fail("RPC response exceeds 1 MiB");
  }
  const newline = buffer.indexOf("\n");
  if (newline < 0) return;
  const line = buffer.slice(0, newline);
  const remainder = buffer.slice(newline + 1).trim();
  if (!line || remainder) return fail("RPC returned more than one response");
  let response;
  try {
    response = JSON.parse(line);
  } catch {
    return fail("RPC response is not valid JSON");
  }
  if (response?.jsonrpc !== "2.0" || response?.id !== expectedId) {
    return fail("RPC response identity mismatch");
  }
  complete = true;
  clearTimeout(timer);
  process.stdout.write(`${JSON.stringify(response)}\n`);
  socket.end();
});
socket.on("end", () => {
  if (!complete) fail("RPC ended without a response");
});
"#;

#[derive(Debug, Error)]
pub enum DockerAdapterError {
    #[error("Docker operation failed")]
    Docker(#[from] bollard::errors::Error),
    #[error("container policy rejected the install record")]
    Policy(#[from] PolicyError),
    #[error("container labels do not match the requested plugin")]
    LabelMismatch,
    #[error("managed image labels do not match the install attestation")]
    ImageLabelMismatch,
    #[error("managed removal attestation does not match the requested plugin")]
    RemovalAttestationMismatch,
    #[error("managed install directory is unsafe")]
    UnsafeInstallDirectory,
    #[error("paired plugin runtime cleanup failed")]
    RuntimeCleanupFailed,
    #[error("startup runtime reconciliation failed for {0} owned resource sets")]
    RuntimeReconcileFailed(usize),
    #[error("plugin proxy token delivery failed")]
    SecretDeliveryFailed,
    #[error("plugin handshake failed: {0}")]
    Handshake(String),
    #[error("plugin is not running")]
    PluginNotRunning,
    #[error("plugin RPC request is invalid")]
    InvalidRpcRequest,
    #[error("plugin RPC timed out")]
    RpcTimeout,
    #[error("plugin RPC exec failed")]
    RpcExecFailed,
    #[error("plugin RPC wrote to stderr")]
    RpcStderr,
    #[error("plugin RPC response is invalid")]
    InvalidRpcResponse,
    #[error("plugin RPC response exceeded the size limit")]
    RpcResponseTooLarge,
}
