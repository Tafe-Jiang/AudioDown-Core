use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use audiodown_domain::plugin::{PluginId, RunMode};
use audiodown_plugin_api::manifest::{PluginManifest, RuntimeKind};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::policy::InstalledPlugin;

const VIRTUAL_IMAGE_ID: &str = "audiodown/plugin-virtual:dev";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyInstallRecord {
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
    pub expected_image_labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManagedInstallRecord {
    installation_id: String,
    operation_id: Uuid,
    plugin_id: PluginId,
    image_id: String,
    source_kind: String,
    repository_id: String,
    commit_sha: String,
    source_hash: String,
    manifest_hash: String,
    base_image_digest: String,
    sdk_hash: String,
    memory_bytes: i64,
    nano_cpus: i64,
    pids_limit: i64,
    run_mode: RunMode,
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
    if let Ok(record) = serde_json::from_slice::<ManagedInstallRecord>(&record_bytes) {
        return load_managed(
            plugin_dir,
            expected_installation_id,
            requested_plugin_id,
            record,
        )
        .await;
    }
    let record: LegacyInstallRecord = serde_json::from_slice(&record_bytes)?;
    load_legacy(
        plugin_dir,
        expected_installation_id,
        requested_plugin_id,
        record,
    )
    .await
}

async fn load_legacy(
    plugin_dir: PathBuf,
    expected_installation_id: &str,
    requested_plugin_id: &PluginId,
    record: LegacyInstallRecord,
) -> Result<ValidatedInstall, InstallRecordError> {
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
        expected_image_labels: None,
    })
}

async fn load_managed(
    plugin_dir: PathBuf,
    expected_installation_id: &str,
    requested_plugin_id: &PluginId,
    record: ManagedInstallRecord,
) -> Result<ValidatedInstall, InstallRecordError> {
    if record.plugin_id != *requested_plugin_id {
        return Err(InstallRecordError::PluginIdMismatch);
    }
    if record.installation_id != expected_installation_id {
        return Err(InstallRecordError::InstallationIdMismatch);
    }
    if record.source_kind != "github_public_repository"
        || record.repository_id.trim().is_empty()
        || !is_lower_hex(&record.commit_sha, 40)
        || !is_lower_hex_sha256(&record.source_hash)
        || !is_lower_hex_sha256(&record.manifest_hash)
        || !is_sha256_digest(&record.base_image_digest)
        || !is_lower_hex_sha256(&record.sdk_hash)
        || record.run_mode != RunMode::OnDemand
        || record.memory_bytes != 128 * 1024 * 1024
        || record.nano_cpus != 500_000_000
        || record.pids_limit != 64
    {
        return Err(InstallRecordError::InvalidManagedAttestation);
    }
    let _ = record.operation_id;

    let manifest_path = plugin_dir.join("audiodown-plugin.json");
    let metadata = tokio::fs::symlink_metadata(&manifest_path)
        .await
        .map_err(|error| InstallRecordError::Read {
            path: manifest_path.clone(),
            error,
        })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(InstallRecordError::ManifestOutsideInstallDirectory);
    }
    let manifest_bytes =
        tokio::fs::read(&manifest_path)
            .await
            .map_err(|error| InstallRecordError::Read {
                path: manifest_path,
                error,
            })?;
    if format!("{:x}", Sha256::digest(&manifest_bytes)) != record.manifest_hash {
        return Err(InstallRecordError::ManifestHashMismatch);
    }
    let manifest = validate_manifest(&manifest_bytes, requested_plugin_id)?;
    let runtime_path = plugin_runtime_path(&manifest.runtime.entry)?;

    let expected_image_labels = HashMap::from([
        ("io.audiodown.managed".to_string(), "true".to_string()),
        (
            "io.audiodown.installation".to_string(),
            record.installation_id.clone(),
        ),
        (
            "io.audiodown.plugin-id".to_string(),
            record.plugin_id.as_str().to_string(),
        ),
        ("io.audiodown.commit-sha".to_string(), record.commit_sha),
        ("io.audiodown.source-hash".to_string(), record.source_hash),
        (
            "io.audiodown.manifest-hash".to_string(),
            record.manifest_hash,
        ),
        (
            "io.audiodown.base-image-digest".to_string(),
            record.base_image_digest,
        ),
        ("io.audiodown.sdk-hash".to_string(), record.sdk_hash),
    ]);

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
        expected_image_labels: Some(expected_image_labels),
    })
}

fn validate_manifest(
    bytes: &[u8],
    requested_plugin_id: &PluginId,
) -> Result<PluginManifest, InstallRecordError> {
    let manifest: PluginManifest = serde_json::from_slice(bytes)?;
    if manifest.id != *requested_plugin_id {
        return Err(InstallRecordError::ManifestIdMismatch);
    }
    if manifest.schema_version != "1.0"
        || manifest.runtime.kind != RuntimeKind::Nodejs
        || manifest.runtime.version != "22"
    {
        return Err(InstallRecordError::RuntimeNotAllowed);
    }
    Ok(manifest)
}

fn is_plain_manifest_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

fn is_lower_hex_sha256(value: &str) -> bool {
    is_lower_hex(value, 64)
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(is_lower_hex_sha256)
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
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
    #[error("managed install attestation is invalid")]
    InvalidManagedAttestation,
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
