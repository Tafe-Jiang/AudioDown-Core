use std::path::{Component, Path, PathBuf};

use audiodown_domain::plugin::{PluginId, RunMode};
use audiodown_plugin_api::manifest::{PluginManifest, RuntimeKind};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::policy::InstalledPlugin;

const VIRTUAL_IMAGE_ID: &str = "audiodown/plugin-virtual:dev";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallRecord {
    plugin_id: PluginId,
    image_id: String,
    manifest_path: PathBuf,
    manifest_hash: String,
    installation_id: String,
    memory_bytes: i64,
    nano_cpus: i64,
    pids_limit: i64,
    run_mode: RunMode,
}

#[derive(Debug, Clone)]
pub struct ValidatedInstall {
    pub installed: InstalledPlugin,
    pub manifest: PluginManifest,
}

pub async fn load(
    plugin_data: &Path,
    expected_installation_id: &str,
    requested_plugin_id: &PluginId,
) -> Result<ValidatedInstall, InstallRecordError> {
    let plugin_dir = plugin_data
        .join("installed")
        .join(requested_plugin_id.as_str());
    let record_path = plugin_dir.join("install.json");
    let record_bytes =
        tokio::fs::read(&record_path)
            .await
            .map_err(|error| InstallRecordError::Read {
                path: record_path.clone(),
                error,
            })?;
    let record: InstallRecord = serde_json::from_slice(&record_bytes)?;

    if record.plugin_id != *requested_plugin_id {
        return Err(InstallRecordError::PluginIdMismatch);
    }
    if record.installation_id != expected_installation_id {
        return Err(InstallRecordError::InstallationIdMismatch);
    }
    if record.image_id != VIRTUAL_IMAGE_ID {
        return Err(InstallRecordError::ImageNotAllowed);
    }
    if record.run_mode != RunMode::OnDemand {
        return Err(InstallRecordError::RunModeNotAllowed);
    }
    if !is_lower_hex_sha256(&record.manifest_hash) {
        return Err(InstallRecordError::InvalidManifestHash);
    }
    if !is_plain_manifest_path(&record.manifest_path) {
        return Err(InstallRecordError::ManifestOutsideInstallDirectory);
    }

    let canonical_plugin_dir = tokio::fs::canonicalize(&plugin_dir)
        .await
        .map_err(|error| InstallRecordError::Read {
            path: plugin_dir.clone(),
            error,
        })?;
    let canonical_manifest = tokio::fs::canonicalize(&record.manifest_path)
        .await
        .map_err(|error| InstallRecordError::Read {
            path: record.manifest_path.clone(),
            error,
        })?;
    if !canonical_manifest.starts_with(&canonical_plugin_dir)
        || canonical_manifest
            .file_name()
            .and_then(|name| name.to_str())
            != Some("audiodown-plugin.json")
    {
        return Err(InstallRecordError::ManifestOutsideInstallDirectory);
    }

    let manifest_bytes = tokio::fs::read(&canonical_manifest)
        .await
        .map_err(|error| InstallRecordError::Read {
            path: canonical_manifest,
            error,
        })?;
    let actual_hash = format!("{:x}", Sha256::digest(&manifest_bytes));
    if actual_hash != record.manifest_hash {
        return Err(InstallRecordError::ManifestHashMismatch);
    }

    let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.id != *requested_plugin_id {
        return Err(InstallRecordError::ManifestIdMismatch);
    }
    if manifest.schema_version != "1.0"
        || manifest.runtime.kind != RuntimeKind::Nodejs
        || manifest.runtime.version != "22"
    {
        return Err(InstallRecordError::RuntimeNotAllowed);
    }
    let runtime_path = plugin_runtime_path(&manifest.runtime.entry)?;

    Ok(ValidatedInstall {
        installed: InstalledPlugin {
            plugin_id: record.plugin_id,
            image_id: record.image_id,
            installation_id: record.installation_id,
            runtime_path,
            memory_bytes: record.memory_bytes,
            nano_cpus: record.nano_cpus,
            pids_limit: record.pids_limit,
        },
        manifest,
    })
}

fn is_plain_manifest_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn plugin_runtime_path(entry: &str) -> Result<String, InstallRecordError> {
    let entry = Path::new(entry);
    if entry.is_absolute()
        || entry.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(InstallRecordError::RuntimeNotAllowed);
    }
    let path = Path::new("/plugin").join(entry);
    Ok(path.to_string_lossy().into_owned())
}

#[derive(Debug, Error)]
pub enum InstallRecordError {
    #[error("failed to read install data at {path}")]
    Read {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
    #[error("install record is invalid")]
    Json(#[from] serde_json::Error),
    #[error("install record plugin ID does not match the request")]
    PluginIdMismatch,
    #[error("install record belongs to another installation")]
    InstallationIdMismatch,
    #[error("install record image is not allowed")]
    ImageNotAllowed,
    #[error("install record run mode is not allowed")]
    RunModeNotAllowed,
    #[error("manifest hash is not a lowercase SHA-256 digest")]
    InvalidManifestHash,
    #[error("manifest path escapes the plugin installation directory")]
    ManifestOutsideInstallDirectory,
    #[error("manifest hash does not match the installed manifest")]
    ManifestHashMismatch,
    #[error("manifest plugin ID does not match the install record")]
    ManifestIdMismatch,
    #[error("manifest runtime is not allowed")]
    RuntimeNotAllowed,
}
