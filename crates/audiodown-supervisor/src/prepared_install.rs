use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use audiodown_domain::plugin::PluginId;
use audiodown_plugin_api::manifest::{PluginManifest, RuntimeKind};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

const SCHEMA_VERSION: &str = "1.0";
const LIFECYCLE_RISK_KIND: &str = "npm_lifecycle_scripts";
const REGISTRY_HOST: &str = "registry.npmjs.org";
const LIFECYCLE_SCRIPTS: [&str; 7] = [
    "preinstall",
    "install",
    "postinstall",
    "prepublish",
    "preprepare",
    "prepare",
    "postprepare",
];

#[derive(Debug, Clone)]
pub struct ValidatedPreparedInstall {
    pub operation_id: Uuid,
    pub snapshot_id: Uuid,
    pub plugin_id: PluginId,
    pub repository_id: String,
    pub source_url: String,
    pub commit_sha: String,
    pub plugin_path: String,
    pub plugin_root: PathBuf,
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub source_hash: String,
    pub allow_lifecycle_scripts: bool,
    pub risk_grant_id: Option<Uuid>,
}

pub async fn validate_prepared_install(
    plugin_data: &Path,
    requested_plugin_id: &PluginId,
    requested_operation_id: Uuid,
) -> Result<ValidatedPreparedInstall, PreparedInstallError> {
    let prepared_path = plugin_data
        .join("prepared")
        .join(format!("{requested_operation_id}.json"));
    let prepared: PreparedMetadata = read_json_regular_file(&prepared_path)?;
    if prepared.schema_version != SCHEMA_VERSION
        || prepared.operation_id != requested_operation_id
        || &prepared.plugin_id != requested_plugin_id
    {
        return Err(PreparedInstallError::IdentityMismatch);
    }
    validate_commit_sha(&prepared.commit_sha)?;
    validate_hash(&prepared.manifest_hash)?;
    validate_hash(&prepared.source_hash)?;

    let snapshot_root = plugin_data
        .join("staging")
        .join(prepared.snapshot_id.to_string());
    require_directory(&snapshot_root)?;
    let snapshot: SnapshotMetadata = read_json_regular_file(&snapshot_root.join("snapshot.json"))?;
    if snapshot.schema_version != SCHEMA_VERSION
        || snapshot.snapshot_id != prepared.snapshot_id
        || snapshot.repository_name.trim().is_empty()
        || snapshot.repository_id != prepared.repository_id
        || snapshot.source_url != prepared.source_url
        || snapshot.commit_sha != prepared.commit_sha
        || snapshot.created_at > Utc::now()
        || snapshot.file_count == 0
        || snapshot.extracted_bytes == 0
    {
        return Err(PreparedInstallError::SnapshotMismatch);
    }
    validate_commit_sha(&snapshot.commit_sha)?;

    let snapshot_plugin = snapshot
        .plugins
        .iter()
        .find(|plugin| plugin.plugin_id == prepared.plugin_id)
        .ok_or(PreparedInstallError::SnapshotMismatch)?;
    if snapshot_plugin.plugin_path != prepared.plugin_path
        || snapshot_plugin.manifest_hash != prepared.manifest_hash
        || snapshot_plugin.source_hash != prepared.source_hash
    {
        return Err(PreparedInstallError::SnapshotMismatch);
    }

    let normalized_plugin_path = normalize_relative_path(&prepared.plugin_path)?;
    let repository_root = snapshot_root.join("repository");
    require_directory(&repository_root)?;
    let plugin_root = repository_root.join(&normalized_plugin_path);
    require_directory(&plugin_root)?;
    validate_regular_tree(&plugin_root)?;

    let manifest_path = plugin_root.join("audiodown-plugin.json");
    let manifest_bytes = read_regular_file(&manifest_path)?;
    if sha256_hex(&manifest_bytes) != prepared.manifest_hash {
        return Err(PreparedInstallError::HashMismatch);
    }
    let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| PreparedInstallError::InvalidManifest)?;
    if manifest.schema_version != SCHEMA_VERSION
        || manifest.id != prepared.plugin_id
        || manifest.name != snapshot_plugin.name
        || manifest.version != snapshot_plugin.version
        || manifest.plugin_type != snapshot_plugin.plugin_type
        || manifest.credentials != snapshot_plugin.credentials
        || manifest.runtime.kind != RuntimeKind::Nodejs
        || manifest.runtime.version != "22"
    {
        return Err(PreparedInstallError::InvalidManifest);
    }
    let runtime_entry = normalize_relative_path(&manifest.runtime.entry)?;
    require_regular_file(&plugin_root.join(runtime_entry))?;

    let source_hash = hash_source_tree(&plugin_root)?;
    if source_hash != prepared.source_hash {
        return Err(PreparedInstallError::HashMismatch);
    }

    let manifest_requires_scripts = manifest.build.npm_lifecycle_scripts.required;
    let manifest_reason = manifest
        .build
        .npm_lifecycle_scripts
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty());
    if snapshot_plugin.requires_lifecycle_scripts != manifest_requires_scripts
        || snapshot_plugin.lifecycle_script_reason.as_deref() != manifest_reason
        || prepared.allow_lifecycle_scripts != manifest_requires_scripts
    {
        return Err(PreparedInstallError::RiskGrantMismatch);
    }

    validate_package(&plugin_root, prepared.allow_lifecycle_scripts)?;
    validate_risk_grant(
        plugin_data,
        &prepared,
        snapshot_plugin.lifecycle_script_reason.as_deref(),
    )?;

    Ok(ValidatedPreparedInstall {
        operation_id: prepared.operation_id,
        snapshot_id: prepared.snapshot_id,
        plugin_id: prepared.plugin_id,
        repository_id: prepared.repository_id,
        source_url: prepared.source_url,
        commit_sha: prepared.commit_sha,
        plugin_path: prepared.plugin_path,
        plugin_root,
        manifest,
        manifest_hash: prepared.manifest_hash,
        source_hash: prepared.source_hash,
        allow_lifecycle_scripts: prepared.allow_lifecycle_scripts,
        risk_grant_id: prepared.risk_grant_id,
    })
}

fn validate_risk_grant(
    plugin_data: &Path,
    prepared: &PreparedMetadata,
    expected_reason: Option<&str>,
) -> Result<(), PreparedInstallError> {
    match (prepared.allow_lifecycle_scripts, prepared.risk_grant_id) {
        (false, None) => Ok(()),
        (true, Some(grant_id)) => {
            let grant: GrantMirror = read_json_regular_file(
                &plugin_data.join("grants").join(format!("{grant_id}.json")),
            )?;
            if grant.schema_version != SCHEMA_VERSION
                || grant.grant_id != grant_id
                || grant.repository_id != prepared.repository_id
                || grant.plugin_id != prepared.plugin_id
                || grant.commit_sha != prepared.commit_sha
                || grant.risk_kind != LIFECYCLE_RISK_KIND
                || Some(grant.reason.as_str()) != expected_reason
                || grant.granted_at > Utc::now()
            {
                return Err(PreparedInstallError::RiskGrantMismatch);
            }
            Ok(())
        }
        _ => Err(PreparedInstallError::RiskGrantMismatch),
    }
}

fn validate_package(
    plugin_root: &Path,
    lifecycle_scripts_allowed: bool,
) -> Result<(), PreparedInstallError> {
    let package = read_json_object(&plugin_root.join("package.json"))?;
    let lockfile = read_json_object(&plugin_root.join("package-lock.json"))?;
    let package_name = required_string(&package, "name")?;
    let package_version = required_string(&package, "version")?;

    let has_lifecycle_scripts = package
        .get("scripts")
        .map(required_object)
        .transpose()?
        .is_some_and(|scripts| {
            LIFECYCLE_SCRIPTS
                .iter()
                .any(|name| scripts.contains_key(*name))
        });
    if has_lifecycle_scripts != lifecycle_scripts_allowed {
        return Err(PreparedInstallError::InvalidPackage);
    }

    if lockfile.get("lockfileVersion").and_then(Value::as_u64) < Some(2) {
        return Err(PreparedInstallError::InvalidPackage);
    }
    let packages = lockfile
        .get("packages")
        .map(required_object)
        .transpose()?
        .ok_or(PreparedInstallError::InvalidPackage)?;
    let root = packages
        .get("")
        .map(required_object)
        .transpose()?
        .ok_or(PreparedInstallError::InvalidPackage)?;
    if required_string(root, "name")? != package_name
        || required_string(root, "version")? != package_version
    {
        return Err(PreparedInstallError::InvalidPackage);
    }
    validate_matching_dependency_maps(&package, root)?;

    if packages.len().saturating_sub(1) > 256 {
        return Err(PreparedInstallError::InvalidPackage);
    }
    for (path, value) in packages {
        if path.is_empty() {
            continue;
        }
        if !path.starts_with("node_modules/") || path.contains('\\') {
            return Err(PreparedInstallError::InvalidPackage);
        }
        let dependency = required_object(value)?;
        if dependency.get("link").and_then(Value::as_bool) == Some(true) {
            return Err(PreparedInstallError::InvalidPackage);
        }
        for field in dependency_fields() {
            dependency_map(dependency, field)?;
        }
        validate_resolved(dependency)?;
        validate_integrity(dependency)?;
    }
    Ok(())
}

fn validate_matching_dependency_maps(
    package: &Map<String, Value>,
    lock_root: &Map<String, Value>,
) -> Result<(), PreparedInstallError> {
    for field in dependency_fields() {
        if dependency_map(package, field)? != dependency_map(lock_root, field)? {
            return Err(PreparedInstallError::InvalidPackage);
        }
    }
    Ok(())
}

fn dependency_fields() -> [&'static str; 4] {
    [
        "dependencies",
        "optionalDependencies",
        "devDependencies",
        "peerDependencies",
    ]
}

fn dependency_map(
    object: &Map<String, Value>,
    field: &str,
) -> Result<BTreeMap<String, String>, PreparedInstallError> {
    let Some(value) = object.get(field) else {
        return Ok(BTreeMap::new());
    };
    let values = required_object(value)?;
    let mut dependencies = BTreeMap::new();
    for (name, value) in values {
        let specification = value
            .as_str()
            .filter(|value| valid_dependency_specification(value))
            .ok_or(PreparedInstallError::InvalidPackage)?;
        if name.is_empty() {
            return Err(PreparedInstallError::InvalidPackage);
        }
        dependencies.insert(name.clone(), specification.to_string());
    }
    Ok(dependencies)
}

fn valid_dependency_specification(value: &str) -> bool {
    let value = value.trim();
    let lowercase = value.to_ascii_lowercase();
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains("://")
        && !lowercase.starts_with("file:")
        && !lowercase.starts_with("link:")
        && !lowercase.starts_with("git")
        && !lowercase.starts_with("github:")
        && !lowercase.starts_with("workspace:")
        && !lowercase.starts_with("http:")
        && !lowercase.starts_with("https:")
}

fn validate_resolved(entry: &Map<String, Value>) -> Result<(), PreparedInstallError> {
    let resolved = required_string(entry, "resolved")?;
    let url = Url::parse(resolved).map_err(|_| PreparedInstallError::InvalidPackage)?;
    if url.scheme() != "https"
        || url.host_str() != Some(REGISTRY_HOST)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PreparedInstallError::InvalidPackage);
    }
    Ok(())
}

fn validate_integrity(entry: &Map<String, Value>) -> Result<(), PreparedInstallError> {
    let encoded = required_string(entry, "integrity")?
        .strip_prefix("sha512-")
        .ok_or(PreparedInstallError::InvalidPackage)?;
    if encoded.contains(char::is_whitespace)
        || STANDARD
            .decode(encoded)
            .map_err(|_| PreparedInstallError::InvalidPackage)?
            .len()
            != 64
    {
        return Err(PreparedInstallError::InvalidPackage);
    }
    Ok(())
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>, PreparedInstallError> {
    let bytes = read_regular_file(path)?;
    serde_json::from_slice::<Value>(&bytes)
        .map_err(|_| PreparedInstallError::InvalidPackage)?
        .as_object()
        .cloned()
        .ok_or(PreparedInstallError::InvalidPackage)
}

fn required_object(value: &Value) -> Result<&Map<String, Value>, PreparedInstallError> {
    value
        .as_object()
        .ok_or(PreparedInstallError::InvalidPackage)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, PreparedInstallError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(PreparedInstallError::InvalidPackage)
}

fn hash_source_tree(root: &Path) -> Result<String, PreparedInstallError> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    for (relative, path) in files {
        let content = read_regular_file(&path)?;
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((content.len() as u64).to_be_bytes());
        hasher.update(content);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), PreparedInstallError> {
    for entry in std::fs::read_dir(directory).map_err(|_| PreparedInstallError::Filesystem)? {
        let entry = entry.map_err(|_| PreparedInstallError::Filesystem)?;
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|_| PreparedInstallError::Filesystem)?;
        if metadata.file_type().is_symlink() {
            return Err(PreparedInstallError::UnsafePath);
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if metadata.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| PreparedInstallError::UnsafePath)?
                .to_str()
                .ok_or(PreparedInstallError::UnsafePath)?
                .replace(std::path::MAIN_SEPARATOR, "/");
            files.push((relative, entry.path()));
        } else {
            return Err(PreparedInstallError::UnsafePath);
        }
    }
    Ok(())
}

fn validate_regular_tree(root: &Path) -> Result<(), PreparedInstallError> {
    collect_files(root, root, &mut Vec::new())
}

fn normalize_relative_path(value: &str) -> Result<PathBuf, PreparedInstallError> {
    if value.is_empty() || value != value.trim() || value.contains('\\') || value.contains('\0') {
        return Err(PreparedInstallError::UnsafePath);
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || value.split('/').any(|segment| segment.contains(':'))
    {
        return Err(PreparedInstallError::UnsafePath);
    }
    Ok(path.to_path_buf())
}

fn require_directory(path: &Path) -> Result<(), PreparedInstallError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| PreparedInstallError::Filesystem)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(PreparedInstallError::UnsafePath);
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<(), PreparedInstallError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| PreparedInstallError::Filesystem)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(PreparedInstallError::UnsafePath);
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, PreparedInstallError> {
    require_regular_file(path)?;
    std::fs::read(path).map_err(|_| PreparedInstallError::Filesystem)
}

fn read_json_regular_file<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<T, PreparedInstallError> {
    serde_json::from_slice(&read_regular_file(path)?)
        .map_err(|_| PreparedInstallError::InvalidMetadata)
}

fn validate_commit_sha(value: &str) -> Result<(), PreparedInstallError> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PreparedInstallError::InvalidMetadata);
    }
    Ok(())
}

fn validate_hash(value: &str) -> Result<(), PreparedInstallError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PreparedInstallError::InvalidMetadata);
    }
    Ok(())
}

fn sha256_hex(content: &[u8]) -> String {
    hex_digest(Sha256::digest(content))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparedMetadata {
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotPlugin {
    plugin_id: PluginId,
    name: String,
    version: semver::Version,
    plugin_type: audiodown_plugin_api::manifest::PluginType,
    #[serde(default)]
    credentials: audiodown_plugin_api::manifest::CredentialDeclarations,
    plugin_path: String,
    manifest_hash: String,
    source_hash: String,
    requires_lifecycle_scripts: bool,
    lifecycle_script_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrantMirror {
    schema_version: String,
    grant_id: Uuid,
    repository_id: String,
    plugin_id: PluginId,
    commit_sha: String,
    risk_kind: String,
    reason: String,
    granted_at: DateTime<Utc>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PreparedInstallError {
    #[error("prepared installation metadata is invalid")]
    InvalidMetadata,
    #[error("prepared installation identity does not match the request")]
    IdentityMismatch,
    #[error("prepared snapshot metadata does not match")]
    SnapshotMismatch,
    #[error("prepared installation contains an unsafe path or file")]
    UnsafePath,
    #[error("prepared installation filesystem access failed")]
    Filesystem,
    #[error("prepared plugin manifest is invalid")]
    InvalidManifest,
    #[error("prepared plugin package metadata is invalid")]
    InvalidPackage,
    #[error("prepared plugin hash does not match")]
    HashMismatch,
    #[error("lifecycle-script risk grant does not match")]
    RiskGrantMismatch,
}
