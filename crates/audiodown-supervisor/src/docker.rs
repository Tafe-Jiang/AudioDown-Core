use std::{collections::HashMap, time::Duration};

use audiodown_domain::plugin::PluginId;
use bollard::{
    exec::{CreateExecOptions, StartExecResults},
    models::{ContainerCreateBody, HostConfig},
    query_parameters::{
        CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
        StopContainerOptionsBuilder,
    },
    Docker,
};
use futures_util::StreamExt;
use serde::Serialize;
use thiserror::Error;

use crate::{
    install_record::ValidatedInstall,
    policy::{PluginContainerPolicy, PolicyError},
};

const RPC_SOCKET: &str = "/tmp/audiodown-rpc.sock";
const HANDSHAKE_ATTEMPTS: usize = 40;
const HANDSHAKE_RETRY_DELAY: Duration = Duration::from_millis(100);

pub struct DockerAdapter {
    docker: Docker,
    installation_id: String,
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

impl DockerAdapter {
    pub fn connect(installation_id: String) -> Result<Self, DockerAdapterError> {
        Ok(Self {
            docker: Docker::connect_with_local_defaults()?,
            installation_id,
        })
    }

    pub async fn start_plugin(
        &self,
        install: ValidatedInstall,
    ) -> Result<StartedPlugin, DockerAdapterError> {
        let plugin_id = install.installed.plugin_id.clone();
        let plugin_version = install.manifest.version.to_string();
        let spec = PluginContainerPolicy::build(install.installed)?;

        if let Some(existing) = self.find_managed_container(&plugin_id).await? {
            self.docker
                .remove_container(
                    &existing,
                    Some(
                        RemoveContainerOptionsBuilder::new()
                            .force(true)
                            .v(true)
                            .build(),
                    ),
                )
                .await?;
        }

        let tmpfs = spec
            .tmpfs
            .iter()
            .filter_map(|entry| entry.split_once(':'))
            .map(|(path, options)| (path.to_string(), options.to_string()))
            .collect();
        let host_config = HostConfig {
            memory: Some(spec.memory_bytes),
            memory_swap: Some(spec.memory_bytes),
            nano_cpus: Some(spec.nano_cpus),
            pids_limit: Some(spec.pids_limit),
            binds: Some(spec.mounts),
            network_mode: Some("none".to_string()),
            privileged: Some(spec.privileged),
            publish_all_ports: Some(false),
            readonly_rootfs: Some(spec.read_only),
            cap_add: Some(spec.cap_add),
            cap_drop: Some(spec.cap_drop),
            security_opt: Some(spec.security_opt),
            tmpfs: Some(tmpfs),
            init: Some(true),
            auto_remove: Some(false),
            ..Default::default()
        };
        let config = ContainerCreateBody {
            image: Some(spec.image_id),
            user: Some("10002:10002".to_string()),
            working_dir: Some("/plugin".to_string()),
            entrypoint: Some(spec.command),
            env: Some(vec![
                format!("AUDIODOWN_RPC_SOCKET={RPC_SOCKET}"),
                "AUDIODOWN_NODE_SDK_PATH=/sdk/src/index.js".to_string(),
                "NODE_ENV=production".to_string(),
            ]),
            labels: Some(spec.labels),
            network_disabled: Some(true),
            attach_stdout: Some(false),
            attach_stderr: Some(false),
            open_stdin: Some(false),
            tty: Some(false),
            host_config: Some(host_config),
            ..Default::default()
        };
        let created = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&spec.container_name)
                        .build(),
                ),
                config,
            )
            .await?;
        self.docker
            .start_container(
                &created.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await?;

        if let Err(error) = self
            .wait_for_handshake(&created.id, &plugin_id, &plugin_version)
            .await
        {
            let _ = self
                .docker
                .remove_container(
                    &created.id,
                    Some(
                        RemoveContainerOptionsBuilder::new()
                            .force(true)
                            .v(true)
                            .build(),
                    ),
                )
                .await;
            return Err(error);
        }

        let logs = self.plugin_logs_by_container(&created.id).await?;
        Ok(StartedPlugin {
            container_id: created.id,
            logs,
        })
    }

    pub async fn stop_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<String>, DockerAdapterError> {
        let Some(container_id) = self.find_managed_container(plugin_id).await? else {
            return Ok(None);
        };
        let inspect = self
            .docker
            .inspect_container(
                &container_id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await?;
        if inspect
            .state
            .and_then(|state| state.running)
            .unwrap_or(false)
        {
            self.docker
                .stop_container(
                    &container_id,
                    Some(StopContainerOptionsBuilder::new().t(5).build()),
                )
                .await?;
        }
        Ok(Some(container_id))
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
        let inspect = self
            .docker
            .inspect_container(
                &container_id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await?;
        Ok(PluginInspection {
            container_id: Some(container_id),
            running: inspect
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

    pub async fn find_managed_container(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<String>, DockerAdapterError> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![
                "io.audiodown.managed=true".to_string(),
                format!("io.audiodown.installation={}", self.installation_id),
                format!("io.audiodown.plugin-id={plugin_id}"),
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

        for container in containers {
            let labels = container.labels.unwrap_or_default();
            if labels.get("io.audiodown.managed").map(String::as_str) != Some("true")
                || labels.get("io.audiodown.installation").map(String::as_str)
                    != Some(self.installation_id.as_str())
                || labels.get("io.audiodown.plugin-id").map(String::as_str)
                    != Some(plugin_id.as_str())
            {
                return Err(DockerAdapterError::LabelMismatch);
            }
            if let Some(id) = container.id {
                return Ok(Some(id));
            }
        }
        Ok(None)
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

#[derive(Debug, Error)]
pub enum DockerAdapterError {
    #[error("Docker operation failed")]
    Docker(#[from] bollard::errors::Error),
    #[error("container policy rejected the install record")]
    Policy(#[from] PolicyError),
    #[error("container labels do not match the requested plugin")]
    LabelMismatch,
    #[error("plugin handshake failed: {0}")]
    Handshake(String),
}
