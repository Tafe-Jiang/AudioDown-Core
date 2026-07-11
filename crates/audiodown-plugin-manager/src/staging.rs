use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::manifest::{PluginManifest, PluginType};
use chrono::{DateTime, Duration, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    archive::ExtractedSnapshot,
    github::GitHubRepositoryRef,
    validation::{ValidatedPlugin, ValidatedRepository},
    PluginManagerError,
};

const SCHEMA_VERSION: &str = "1.0";
const SNAPSHOT_TTL_MINUTES: i64 = 30;
const LIFECYCLE_RISK_KIND: &str = "npm_lifecycle_scripts";

pub struct SnapshotStore {
    plugin_data: PathBuf,
}

impl SnapshotStore {
    pub fn new(plugin_data: impl Into<PathBuf>) -> Self {
        Self {
            plugin_data: plugin_data.into(),
        }
    }

    pub fn plugin_data(&self) -> &Path {
        &self.plugin_data
    }

    pub async fn create(
        &self,
        source: &GitHubRepositoryRef,
        commit_sha: &str,
        extracted: ExtractedSnapshot,
        validated: ValidatedRepository,
    ) -> Result<RepositoryPreview, PluginManagerError> {
        let snapshot_id = Uuid::new_v4();
        let staging_root = self.plugin_data.join("staging");
        create_secure_directory_all(&staging_root)?;

        let temporary_root = staging_root.join(format!(".{snapshot_id}.tmp"));
        let snapshot_root = staging_root.join(snapshot_id.to_string());
        create_secure_directory(&temporary_root)?;

        let result = (|| {
            let repository_root = temporary_root.join("repository");
            std::fs::rename(&extracted.repository_root, &repository_root)
                .map_err(|_| PluginManagerError::SnapshotIo)?;
            secure_tree(&repository_root)?;

            let created_at = Utc::now();
            let metadata = SnapshotMetadata {
                schema_version: SCHEMA_VERSION.to_string(),
                snapshot_id,
                repository_id: validated.repository_id.clone(),
                repository_name: validated.repository_name.clone(),
                source_url: source.canonical_url().to_string(),
                commit_sha: commit_sha.to_string(),
                created_at,
                file_count: extracted.file_count,
                extracted_bytes: extracted.extracted_bytes,
                plugins: validated.plugins.iter().map(SnapshotPlugin::from).collect(),
            };
            atomic_write_json(&temporary_root.join("snapshot.json"), &metadata)?;
            sync_directory(&temporary_root)?;
            std::fs::rename(&temporary_root, &snapshot_root)
                .map_err(|_| PluginManagerError::SnapshotIo)?;
            sync_directory(&staging_root)?;

            Ok(RepositoryPreview {
                snapshot_id,
                repository_id: validated.repository_id,
                repository_name: validated.repository_name,
                source_url: source.canonical_url().to_string(),
                commit_sha: commit_sha.to_string(),
                plugins: validated
                    .plugins
                    .into_iter()
                    .map(PluginPreview::from)
                    .collect(),
            })
        })();

        if result.is_err() {
            let _ = std::fs::remove_dir_all(&temporary_root);
        }
        result
    }

    pub async fn prepare_install(
        &self,
        snapshot_id: Uuid,
        plugin_id: &PluginId,
        grant: Option<&LifecycleRiskGrant>,
    ) -> Result<PreparedOperation, PluginManagerError> {
        let metadata = self.load_snapshot(snapshot_id)?;
        let plugin = metadata
            .plugins
            .iter()
            .find(|candidate| &candidate.plugin_id == plugin_id)
            .ok_or(PluginManagerError::InvalidStagingMetadata)?;
        validate_grant(&metadata, plugin, grant)?;

        let operation_id = Uuid::new_v4();
        let risk_grant_id = grant.map(|value| value.id);
        let operation = PreparedOperationMetadata {
            schema_version: SCHEMA_VERSION.to_string(),
            operation_id,
            snapshot_id,
            plugin_id: plugin_id.clone(),
            repository_id: metadata.repository_id.clone(),
            source_url: metadata.source_url.clone(),
            commit_sha: metadata.commit_sha.clone(),
            plugin_path: plugin.plugin_path.clone(),
            manifest_hash: plugin.manifest_hash.clone(),
            source_hash: plugin.source_hash.clone(),
            allow_lifecycle_scripts: plugin.requires_lifecycle_scripts,
            risk_grant_id,
        };

        let prepared_root = self.plugin_data.join("prepared");
        create_secure_directory_all(&prepared_root)?;
        let mut mirrored_grant = None;
        if let Some(grant) = grant {
            let grants_root = self.plugin_data.join("grants");
            create_secure_directory_all(&grants_root)?;
            let grant_path = grants_root.join(format!("{}.json", grant.id));
            atomic_write_json(&grant_path, &GrantMirror::from(grant))?;
            mirrored_grant = Some(grant_path);
        }

        let operation_path = prepared_root.join(format!("{operation_id}.json"));
        if let Err(error) = atomic_write_json(&operation_path, &operation) {
            if let Some(path) = mirrored_grant {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }

        Ok(PreparedOperation {
            operation_id,
            plugin_id: plugin_id.clone(),
        })
    }

    pub fn load_plugin(
        &self,
        snapshot_id: Uuid,
        plugin_id: &PluginId,
        now: DateTime<Utc>,
    ) -> Result<StagedPlugin, PluginManagerError> {
        let metadata = self.load_snapshot(snapshot_id)?;
        if now.signed_duration_since(metadata.created_at) > Duration::minutes(SNAPSHOT_TTL_MINUTES)
        {
            return Err(PluginManagerError::SnapshotNotFound);
        }
        let plugin = metadata
            .plugins
            .into_iter()
            .find(|candidate| &candidate.plugin_id == plugin_id)
            .ok_or(PluginManagerError::InvalidStagingMetadata)?;
        let manifest_path = self
            .plugin_data
            .join("staging")
            .join(snapshot_id.to_string())
            .join("repository")
            .join(&plugin.plugin_path)
            .join("audiodown-plugin.json");
        let manifest_bytes =
            std::fs::read(manifest_path).map_err(|_| PluginManagerError::SnapshotIo)?;
        let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| PluginManagerError::InvalidPluginManifest)?;
        if manifest.id != plugin.plugin_id
            || manifest.name != plugin.name
            || manifest.version != plugin.version
            || manifest.plugin_type != plugin.plugin_type
        {
            return Err(PluginManagerError::InvalidStagingMetadata);
        }

        Ok(StagedPlugin {
            snapshot_id,
            repository_id: metadata.repository_id,
            source_url: metadata.source_url,
            commit_sha: metadata.commit_sha,
            plugin_path: plugin.plugin_path,
            manifest,
            manifest_hash: plugin.manifest_hash,
            source_hash: plugin.source_hash,
            requires_lifecycle_scripts: plugin.requires_lifecycle_scripts,
            lifecycle_script_reason: plugin.lifecycle_script_reason,
        })
    }

    pub async fn cleanup_expired(&self, now: DateTime<Utc>) -> Result<(), PluginManagerError> {
        let staging_root = self.plugin_data.join("staging");
        if !staging_root.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&staging_root).map_err(|_| PluginManagerError::SnapshotIo)? {
            let entry = entry.map_err(|_| PluginManagerError::SnapshotIo)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if Uuid::parse_str(name).is_err() {
                continue;
            }

            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|_| PluginManagerError::SnapshotIo)?;
            if metadata.file_type().is_symlink() {
                std::fs::remove_file(entry.path()).map_err(|_| PluginManagerError::SnapshotIo)?;
                continue;
            }
            if !metadata.is_dir() {
                continue;
            }

            let snapshot_path = entry.path().join("snapshot.json");
            let snapshot_file = match std::fs::symlink_metadata(&snapshot_path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    metadata
                }
                _ => continue,
            };
            let _ = snapshot_file;
            let bytes = std::fs::read(snapshot_path).map_err(|_| PluginManagerError::SnapshotIo)?;
            let cleanup: CleanupMetadata = serde_json::from_slice(&bytes)
                .map_err(|_| PluginManagerError::InvalidStagingMetadata)?;
            if now.signed_duration_since(cleanup.created_at)
                > Duration::minutes(SNAPSHOT_TTL_MINUTES)
            {
                std::fs::remove_dir_all(entry.path())
                    .map_err(|_| PluginManagerError::SnapshotIo)?;
            }
        }
        sync_directory(&staging_root)
    }

    fn load_snapshot(&self, snapshot_id: Uuid) -> Result<SnapshotMetadata, PluginManagerError> {
        let snapshot_root = self
            .plugin_data
            .join("staging")
            .join(snapshot_id.to_string());
        let root_metadata = std::fs::symlink_metadata(&snapshot_root)
            .map_err(|_| PluginManagerError::SnapshotNotFound)?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err(PluginManagerError::InvalidStagingMetadata);
        }

        let metadata_path = snapshot_root.join("snapshot.json");
        let file_metadata = std::fs::symlink_metadata(&metadata_path)
            .map_err(|_| PluginManagerError::SnapshotNotFound)?;
        if !file_metadata.is_file() || file_metadata.file_type().is_symlink() {
            return Err(PluginManagerError::InvalidStagingMetadata);
        }

        let bytes = std::fs::read(metadata_path).map_err(|_| PluginManagerError::SnapshotIo)?;
        let metadata: SnapshotMetadata = serde_json::from_slice(&bytes)
            .map_err(|_| PluginManagerError::InvalidStagingMetadata)?;
        if metadata.schema_version != SCHEMA_VERSION || metadata.snapshot_id != snapshot_id {
            return Err(PluginManagerError::InvalidStagingMetadata);
        }
        Ok(metadata)
    }
}

#[derive(Debug, Clone)]
pub struct LifecycleRiskGrant {
    pub id: Uuid,
    pub repository_id: String,
    pub plugin_id: PluginId,
    pub commit_sha: String,
    pub risk_kind: String,
    pub reason: String,
    pub granted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPreview {
    pub snapshot_id: Uuid,
    pub repository_id: String,
    pub repository_name: String,
    pub source_url: String,
    pub commit_sha: String,
    pub plugins: Vec<PluginPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPreview {
    pub plugin_id: PluginId,
    pub name: String,
    pub version: Version,
    pub plugin_type: PluginType,
    pub requires_lifecycle_script_grant: bool,
    pub lifecycle_script_reason: Option<String>,
}

impl From<ValidatedPlugin> for PluginPreview {
    fn from(value: ValidatedPlugin) -> Self {
        Self {
            plugin_id: value.manifest.id,
            name: value.manifest.name,
            version: value.manifest.version,
            plugin_type: value.manifest.plugin_type,
            requires_lifecycle_script_grant: value.requires_lifecycle_scripts,
            lifecycle_script_reason: value.lifecycle_script_reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedOperation {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
}

#[derive(Debug, Clone)]
pub struct StagedPlugin {
    pub snapshot_id: Uuid,
    pub repository_id: String,
    pub source_url: String,
    pub commit_sha: String,
    pub plugin_path: String,
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub source_hash: String,
    pub requires_lifecycle_scripts: bool,
    pub lifecycle_script_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotMetadata {
    schema_version: String,
    snapshot_id: Uuid,
    repository_id: String,
    repository_name: String,
    source_url: String,
    commit_sha: String,
    created_at: DateTime<Utc>,
    file_count: usize,
    extracted_bytes: u64,
    plugins: Vec<SnapshotPlugin>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotPlugin {
    plugin_id: PluginId,
    name: String,
    version: Version,
    plugin_type: PluginType,
    plugin_path: String,
    manifest_hash: String,
    source_hash: String,
    requires_lifecycle_scripts: bool,
    lifecycle_script_reason: Option<String>,
}

impl From<&ValidatedPlugin> for SnapshotPlugin {
    fn from(value: &ValidatedPlugin) -> Self {
        Self {
            plugin_id: value.manifest.id.clone(),
            name: value.manifest.name.clone(),
            version: value.manifest.version.clone(),
            plugin_type: value.manifest.plugin_type,
            plugin_path: value.relative_path.clone(),
            manifest_hash: value.manifest_hash.clone(),
            source_hash: value.source_hash.clone(),
            requires_lifecycle_scripts: value.requires_lifecycle_scripts,
            lifecycle_script_reason: value.lifecycle_script_reason.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedOperationMetadata {
    schema_version: String,
    operation_id: Uuid,
    snapshot_id: Uuid,
    plugin_id: PluginId,
    repository_id: String,
    source_url: String,
    commit_sha: String,
    plugin_path: String,
    manifest_hash: String,
    source_hash: String,
    allow_lifecycle_scripts: bool,
    risk_grant_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantMirror<'a> {
    schema_version: &'static str,
    grant_id: Uuid,
    repository_id: &'a str,
    plugin_id: &'a PluginId,
    commit_sha: &'a str,
    risk_kind: &'a str,
    reason: &'a str,
    granted_at: DateTime<Utc>,
}

impl<'a> From<&'a LifecycleRiskGrant> for GrantMirror<'a> {
    fn from(value: &'a LifecycleRiskGrant) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            grant_id: value.id,
            repository_id: &value.repository_id,
            plugin_id: &value.plugin_id,
            commit_sha: &value.commit_sha,
            risk_kind: &value.risk_kind,
            reason: &value.reason,
            granted_at: value.granted_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanupMetadata {
    created_at: DateTime<Utc>,
}

fn validate_grant(
    snapshot: &SnapshotMetadata,
    plugin: &SnapshotPlugin,
    grant: Option<&LifecycleRiskGrant>,
) -> Result<(), PluginManagerError> {
    match (plugin.requires_lifecycle_scripts, grant) {
        (false, None) => Ok(()),
        (true, Some(grant))
            if grant.repository_id == snapshot.repository_id
                && grant.plugin_id == plugin.plugin_id
                && grant.commit_sha == snapshot.commit_sha
                && grant.risk_kind == LIFECYCLE_RISK_KIND
                && plugin.lifecycle_script_reason.as_deref() == Some(grant.reason.as_str()) =>
        {
            Ok(())
        }
        _ => Err(PluginManagerError::RiskGrantMismatch),
    }
}

fn atomic_write_json<T: Serialize>(
    destination: &Path,
    value: &T,
) -> Result<(), PluginManagerError> {
    let parent = destination.parent().ok_or(PluginManagerError::SnapshotIo)?;
    create_secure_directory_all(parent)?;
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(PluginManagerError::SnapshotIo)?;
    let temporary_path = parent.join(format!(".{file_name}.tmp"));

    let result = (|| {
        let mut file = secure_file(&temporary_path)?;
        serde_json::to_writer(&mut file, value)
            .map_err(|_| PluginManagerError::InvalidStagingMetadata)?;
        file.write_all(b"\n")
            .map_err(|_| PluginManagerError::SnapshotIo)?;
        file.sync_all()
            .map_err(|_| PluginManagerError::SnapshotIo)?;
        drop(file);
        std::fs::rename(&temporary_path, destination)
            .map_err(|_| PluginManagerError::SnapshotIo)?;
        sync_directory(parent)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn secure_tree(root: &Path) -> Result<(), PluginManagerError> {
    set_directory_mode(root)?;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(|_| PluginManagerError::SnapshotIo)? {
            let entry = entry.map_err(|_| PluginManagerError::SnapshotIo)?;
            let file_type = entry
                .file_type()
                .map_err(|_| PluginManagerError::SnapshotIo)?;
            if file_type.is_symlink() {
                return Err(PluginManagerError::InvalidStagingMetadata);
            }
            if file_type.is_dir() {
                set_directory_mode(&entry.path())?;
                pending.push(entry.path());
            } else if file_type.is_file() {
                set_file_mode(&entry.path())?;
            } else {
                return Err(PluginManagerError::InvalidStagingMetadata);
            }
        }
    }
    Ok(())
}

fn create_secure_directory_all(path: &Path) -> Result<(), PluginManagerError> {
    std::fs::create_dir_all(path).map_err(|_| PluginManagerError::SnapshotIo)?;
    set_directory_mode(path)
}

fn create_secure_directory(path: &Path) -> Result<(), PluginManagerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|_| PluginManagerError::SnapshotIo)?;
        set_directory_mode(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path).map_err(|_| PluginManagerError::SnapshotIo)
    }
}

fn secure_file(path: &Path) -> Result<File, PluginManagerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| PluginManagerError::SnapshotIo)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| PluginManagerError::SnapshotIo)
    }
}

fn sync_directory(path: &Path) -> Result<(), PluginManagerError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| PluginManagerError::SnapshotIo)
}

#[cfg(unix)]
fn set_directory_mode(path: &Path) -> Result<(), PluginManagerError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| PluginManagerError::SnapshotIo)
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path) -> Result<(), PluginManagerError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(path: &Path) -> Result<(), PluginManagerError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| PluginManagerError::SnapshotIo)
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path) -> Result<(), PluginManagerError> {
    Ok(())
}
