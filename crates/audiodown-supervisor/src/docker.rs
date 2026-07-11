use std::collections::HashMap;

use audiodown_domain::plugin::PluginId;
use bollard::{query_parameters::ListContainersOptionsBuilder, Docker};
use thiserror::Error;

pub struct DockerAdapter {
    docker: Docker,
    installation_id: String,
}

impl DockerAdapter {
    pub fn connect(installation_id: String) -> Result<Self, DockerAdapterError> {
        Ok(Self {
            docker: Docker::connect_with_local_defaults()?,
            installation_id,
        })
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
                ListContainersOptionsBuilder::new()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await?;

        for container in containers {
            let labels = container.labels.unwrap_or_default();
            if labels.get("io.audiodown.managed").map(String::as_str) != Some("true")
                || labels
                    .get("io.audiodown.installation")
                    .map(String::as_str)
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
}

#[derive(Debug, Error)]
pub enum DockerAdapterError {
    #[error("Docker operation failed")]
    Docker(#[from] bollard::errors::Error),
    #[error("container labels do not match the requested plugin")]
    LabelMismatch,
}
