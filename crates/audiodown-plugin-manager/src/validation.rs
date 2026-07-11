use std::{
    collections::HashSet,
    net::IpAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::OnceLock,
};

use audiodown_plugin_api::{
    manifest::{capability_is_supported, PluginManifest},
    repository::RepositoryIndex,
};
use regex::Regex;
use semver::{BuildMetadata, Prerelease, Version, VersionReq};
use sha2::{Digest, Sha256};

use crate::{archive::SnapshotLimits, package::validate_package, PluginManagerError};

#[derive(Debug, Clone)]
pub struct ValidatedRepository {
    pub repository_id: String,
    pub repository_name: String,
    pub plugins: Vec<ValidatedPlugin>,
}

#[derive(Debug, Clone)]
pub struct ValidatedPlugin {
    pub relative_path: String,
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub source_hash: String,
    pub entry_path: String,
    pub requires_lifecycle_scripts: bool,
    pub lifecycle_script_reason: Option<String>,
}

pub fn validate_repository(
    repository_root: &Path,
    core_version: &Version,
    plugin_api_version: &Version,
    limits: SnapshotLimits,
) -> Result<ValidatedRepository, PluginManagerError> {
    let index_bytes = std::fs::read(repository_root.join("audiodown-repository.json"))
        .map_err(|_| PluginManagerError::InvalidRepositoryIndex)?;
    let index: RepositoryIndex = serde_json::from_slice(&index_bytes)
        .map_err(|_| PluginManagerError::InvalidRepositoryIndex)?;
    validate_repository_metadata(&index)?;
    if index.plugins.len() > limits.max_plugins {
        return Err(PluginManagerError::InvalidRepositoryIndex);
    }

    let normalized_core = compatibility_version(core_version);
    let normalized_plugin_api = compatibility_version(plugin_api_version);
    let mut plugin_paths = HashSet::new();
    let mut plugin_ids = HashSet::new();
    let mut plugins = Vec::with_capacity(index.plugins.len());

    for plugin_reference in index.plugins {
        let (relative_path, relative_path_buf) = normalize_relative_path(&plugin_reference.path)?;
        if !plugin_paths.insert(relative_path.to_ascii_lowercase()) {
            return Err(PluginManagerError::InvalidPluginPath);
        }

        let plugin_root = repository_root.join(&relative_path_buf);
        let metadata = std::fs::symlink_metadata(&plugin_root)
            .map_err(|_| PluginManagerError::InvalidPluginPath)?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(PluginManagerError::InvalidPluginPath);
        }
        validate_plugin_tree(&plugin_root)?;

        let manifest_path = plugin_root.join("audiodown-plugin.json");
        let manifest_bytes =
            std::fs::read(&manifest_path).map_err(|_| PluginManagerError::InvalidPluginManifest)?;
        let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| PluginManagerError::InvalidPluginManifest)?;
        validate_manifest(
            &manifest,
            &plugin_root,
            &normalized_core,
            &normalized_plugin_api,
        )?;
        if !plugin_ids.insert(manifest.id.as_str().to_string()) {
            return Err(PluginManagerError::DuplicatePlugin);
        }

        let (entry_path, entry_path_buf) = normalize_relative_path(&manifest.runtime.entry)?;
        let entry_metadata = std::fs::symlink_metadata(plugin_root.join(&entry_path_buf))
            .map_err(|_| PluginManagerError::InvalidPluginManifest)?;
        if !entry_metadata.file_type().is_file() || entry_metadata.file_type().is_symlink() {
            return Err(PluginManagerError::InvalidPluginManifest);
        }

        let requires_lifecycle_scripts = manifest.build.npm_lifecycle_scripts.required;
        let reason = manifest
            .build
            .npm_lifecycle_scripts
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(str::to_string);
        validate_package(&plugin_root, requires_lifecycle_scripts)?;

        plugins.push(ValidatedPlugin {
            relative_path,
            manifest,
            manifest_hash: sha256_hex(&manifest_bytes),
            source_hash: hash_source_tree(&plugin_root)?,
            entry_path,
            requires_lifecycle_scripts,
            lifecycle_script_reason: reason,
        });
    }

    Ok(ValidatedRepository {
        repository_id: index.repository.id,
        repository_name: index.repository.name,
        plugins,
    })
}

fn validate_repository_metadata(index: &RepositoryIndex) -> Result<(), PluginManagerError> {
    if index.schema_version != "1.0" || !repository_id_pattern().is_match(&index.repository.id) {
        return Err(PluginManagerError::InvalidRepositoryIndex);
    }
    let name = index.repository.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(PluginManagerError::InvalidRepositoryIndex);
    }
    Ok(())
}

fn validate_manifest(
    manifest: &PluginManifest,
    plugin_root: &Path,
    core_version: &Version,
    plugin_api_version: &Version,
) -> Result<(), PluginManagerError> {
    if manifest.schema_version != "1.0" || manifest.runtime.version != "22" {
        return Err(PluginManagerError::InvalidPluginManifest);
    }

    let core_requirement = parse_version_requirement(&manifest.compatibility.core)?;
    let plugin_api_requirement = parse_version_requirement(&manifest.compatibility.plugin_api)?;
    if !core_requirement.matches(core_version)
        || !plugin_api_requirement.matches(plugin_api_version)
    {
        return Err(PluginManagerError::IncompatiblePlugin);
    }

    let mut capabilities = HashSet::new();
    for capability in &manifest.capabilities {
        if !capability_pattern().is_match(capability)
            || !capabilities.insert(capability)
            || !capability_is_supported(manifest.plugin_type, capability)
        {
            return Err(PluginManagerError::InvalidPluginManifest);
        }
    }

    let mut allowed_hosts = HashSet::new();
    for host in &manifest.network.allowed_hosts {
        if !valid_allowed_host(host) || !allowed_hosts.insert(host) {
            return Err(PluginManagerError::InvalidPluginManifest);
        }
    }

    let policy = &manifest.build.npm_lifecycle_scripts;
    if policy.required {
        let reason = policy
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .ok_or(PluginManagerError::InvalidPluginManifest)?;
        if reason.chars().count() > 240 {
            return Err(PluginManagerError::InvalidPluginManifest);
        }
    }

    let (_, entry_path) = normalize_relative_path(&manifest.runtime.entry)?;
    if !plugin_root.join(entry_path).is_file() {
        return Err(PluginManagerError::InvalidPluginManifest);
    }
    Ok(())
}

fn validate_plugin_tree(plugin_root: &Path) -> Result<(), PluginManagerError> {
    let mut pending = vec![(plugin_root.to_path_buf(), 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        let entries =
            std::fs::read_dir(&directory).map_err(|_| PluginManagerError::ForbiddenPluginFile)?;
        for entry in entries {
            let entry = entry.map_err(|_| PluginManagerError::ForbiddenPluginFile)?;
            let file_type = entry
                .file_type()
                .map_err(|_| PluginManagerError::ForbiddenPluginFile)?;
            if file_type.is_symlink() {
                return Err(PluginManagerError::ForbiddenPluginFile);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| PluginManagerError::ForbiddenPluginFile)?;
            if forbidden_name(&name)
                || (depth > 0 && matches!(name.as_str(), "package.json" | "package-lock.json"))
            {
                return Err(PluginManagerError::ForbiddenPluginFile);
            }
            if file_type.is_dir() {
                pending.push((entry.path(), depth + 1));
            } else if !file_type.is_file() {
                return Err(PluginManagerError::ForbiddenPluginFile);
            }
        }
    }
    Ok(())
}

fn forbidden_name(name: &str) -> bool {
    matches!(
        name,
        ".npmrc"
            | "npm-shrinkwrap.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "Dockerfile"
            | ".dockerignore"
    )
}

fn normalize_relative_path(value: &str) -> Result<(String, PathBuf), PluginManagerError> {
    if value.is_empty()
        || value != value.trim()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(PluginManagerError::InvalidPluginPath);
    }
    let mut normalized = PathBuf::new();
    let mut segments = Vec::new();
    for segment in value.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") || segment.contains(':') {
            return Err(PluginManagerError::InvalidPluginPath);
        }
        normalized.push(segment);
        segments.push(segment);
    }
    Ok((segments.join("/"), normalized))
}

fn compatibility_version(version: &Version) -> Version {
    let mut normalized = version.clone();
    normalized.pre = Prerelease::EMPTY;
    normalized.build = BuildMetadata::EMPTY;
    normalized
}

fn parse_version_requirement(value: &str) -> Result<VersionReq, PluginManagerError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(PluginManagerError::InvalidPluginManifest);
    }
    VersionReq::parse(value)
        .or_else(|_| VersionReq::parse(&value.split_whitespace().collect::<Vec<_>>().join(", ")))
        .map_err(|_| PluginManagerError::InvalidPluginManifest)
}

fn valid_allowed_host(value: &str) -> bool {
    if value.is_empty() || value != value.to_ascii_lowercase() || value.len() > 253 {
        return false;
    }
    let host = value.strip_prefix("*.").unwrap_or(value);
    if host.contains('*')
        || host == "localhost"
        || host.ends_with(".localhost")
        || IpAddr::from_str(host).is_ok()
        || !host.contains('.')
    {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

fn hash_source_tree(plugin_root: &Path) -> Result<String, PluginManagerError> {
    let mut files = Vec::new();
    collect_files(plugin_root, plugin_root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    for (relative_path, absolute_path) in files {
        let content =
            std::fs::read(absolute_path).map_err(|_| PluginManagerError::InvalidPluginManifest)?;
        let path_bytes = relative_path.as_bytes();
        hasher.update((path_bytes.len() as u64).to_be_bytes());
        hasher.update(path_bytes);
        hasher.update((content.len() as u64).to_be_bytes());
        hasher.update(content);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), PluginManagerError> {
    for entry in
        std::fs::read_dir(directory).map_err(|_| PluginManagerError::InvalidPluginManifest)?
    {
        let entry = entry.map_err(|_| PluginManagerError::InvalidPluginManifest)?;
        let file_type = entry
            .file_type()
            .map_err(|_| PluginManagerError::InvalidPluginManifest)?;
        if file_type.is_symlink() {
            return Err(PluginManagerError::ForbiddenPluginFile);
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| PluginManagerError::InvalidPluginManifest)?
                .to_str()
                .ok_or(PluginManagerError::InvalidPluginManifest)?
                .replace(std::path::MAIN_SEPARATOR, "/");
            files.push((relative, entry.path()));
        } else {
            return Err(PluginManagerError::ForbiddenPluginFile);
        }
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

fn repository_id_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^[a-z0-9](?:[a-z0-9._-]{0,126}[a-z0-9])?$").expect("valid repository ID regex")
    })
}

fn capability_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)+$").expect("valid capability regex")
    })
}
