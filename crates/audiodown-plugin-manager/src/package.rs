use std::{collections::BTreeMap, path::Path};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{Map, Value};
use url::Url;

use crate::PluginManagerError;

const LIFECYCLE_SCRIPTS: [&str; 7] = [
    "preinstall",
    "install",
    "postinstall",
    "prepublish",
    "preprepare",
    "prepare",
    "postprepare",
];

pub(crate) fn validate_package(
    plugin_root: &Path,
    lifecycle_scripts_required: bool,
) -> Result<(), PluginManagerError> {
    let package = read_object(&plugin_root.join("package.json"))?;
    let lockfile = read_object(&plugin_root.join("package-lock.json"))?;

    let package_name = required_string(&package, "name")?;
    let package_version = required_string(&package, "version")?;
    let has_lifecycle_scripts =
        package
            .get("scripts")
            .map(object)
            .transpose()?
            .is_some_and(|scripts| {
                LIFECYCLE_SCRIPTS
                    .iter()
                    .any(|name| scripts.contains_key(*name))
            });
    if has_lifecycle_scripts != lifecycle_scripts_required {
        return Err(PluginManagerError::InvalidPackage);
    }

    let lockfile_version = lockfile
        .get("lockfileVersion")
        .and_then(Value::as_u64)
        .ok_or(PluginManagerError::InvalidPackage)?;
    if lockfile_version < 2 {
        return Err(PluginManagerError::InvalidPackage);
    }

    let packages = lockfile
        .get("packages")
        .map(object)
        .transpose()?
        .ok_or(PluginManagerError::InvalidPackage)?;
    let root_package = packages
        .get("")
        .map(object)
        .transpose()?
        .ok_or(PluginManagerError::InvalidPackage)?;
    if required_string(root_package, "name")? != package_name
        || required_string(root_package, "version")? != package_version
    {
        return Err(PluginManagerError::InvalidPackage);
    }

    validate_root_dependencies(&package, root_package)?;

    let locked_packages = packages.len().saturating_sub(1);
    if locked_packages > 256 {
        return Err(PluginManagerError::InvalidPackage);
    }
    for (path, entry) in packages {
        if path.is_empty() {
            continue;
        }
        if !path.starts_with("node_modules/") || path.contains('\\') {
            return Err(PluginManagerError::InvalidPackage);
        }
        let entry = object(entry)?;
        if entry.get("link").and_then(Value::as_bool) == Some(true) {
            return Err(PluginManagerError::InvalidPackage);
        }
        validate_dependency_maps(entry)?;
        validate_resolved(entry)?;
        validate_integrity(entry)?;
    }

    Ok(())
}

fn read_object(path: &Path) -> Result<Map<String, Value>, PluginManagerError> {
    let bytes = std::fs::read(path).map_err(|_| PluginManagerError::InvalidPackage)?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| PluginManagerError::InvalidPackage)?;
    value
        .as_object()
        .cloned()
        .ok_or(PluginManagerError::InvalidPackage)
}

fn object(value: &Value) -> Result<&Map<String, Value>, PluginManagerError> {
    value.as_object().ok_or(PluginManagerError::InvalidPackage)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, PluginManagerError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(PluginManagerError::InvalidPackage)
}

fn validate_root_dependencies(
    package: &Map<String, Value>,
    lock_root: &Map<String, Value>,
) -> Result<(), PluginManagerError> {
    for field in [
        "dependencies",
        "optionalDependencies",
        "devDependencies",
        "peerDependencies",
    ] {
        let package_dependencies = dependency_map(package, field)?;
        let lock_dependencies = dependency_map(lock_root, field)?;
        if package_dependencies != lock_dependencies {
            return Err(PluginManagerError::InvalidPackage);
        }
    }
    Ok(())
}

fn validate_dependency_maps(object: &Map<String, Value>) -> Result<(), PluginManagerError> {
    for field in [
        "dependencies",
        "optionalDependencies",
        "devDependencies",
        "peerDependencies",
    ] {
        dependency_map(object, field)?;
    }
    Ok(())
}

fn dependency_map(
    object: &Map<String, Value>,
    field: &str,
) -> Result<BTreeMap<String, String>, PluginManagerError> {
    let Some(value) = object.get(field) else {
        return Ok(BTreeMap::new());
    };
    let dependencies = value
        .as_object()
        .ok_or(PluginManagerError::InvalidPackage)?;
    let mut normalized = BTreeMap::new();
    for (name, value) in dependencies {
        if name.is_empty() {
            return Err(PluginManagerError::InvalidPackage);
        }
        let specification = value.as_str().ok_or(PluginManagerError::InvalidPackage)?;
        validate_dependency_specification(specification)?;
        normalized.insert(name.clone(), specification.to_string());
    }
    Ok(normalized)
}

fn validate_dependency_specification(value: &str) -> Result<(), PluginManagerError> {
    let value = value.trim();
    let lowercase = value.to_ascii_lowercase();
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.contains("://")
        || lowercase.starts_with("file:")
        || lowercase.starts_with("link:")
        || lowercase.starts_with("git")
        || lowercase.starts_with("github:")
        || lowercase.starts_with("workspace:")
        || lowercase.starts_with("http:")
        || lowercase.starts_with("https:")
    {
        return Err(PluginManagerError::InvalidPackage);
    }
    Ok(())
}

fn validate_resolved(entry: &Map<String, Value>) -> Result<(), PluginManagerError> {
    let resolved = required_string(entry, "resolved")?;
    let url = Url::parse(resolved).map_err(|_| PluginManagerError::InvalidPackage)?;
    if url.scheme() != "https"
        || url.host_str() != Some("registry.npmjs.org")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PluginManagerError::InvalidPackage);
    }
    Ok(())
}

fn validate_integrity(entry: &Map<String, Value>) -> Result<(), PluginManagerError> {
    let integrity = required_string(entry, "integrity")?;
    let encoded = integrity
        .strip_prefix("sha512-")
        .ok_or(PluginManagerError::InvalidPackage)?;
    if encoded.contains(char::is_whitespace) {
        return Err(PluginManagerError::InvalidPackage);
    }
    let digest = STANDARD
        .decode(encoded)
        .map_err(|_| PluginManagerError::InvalidPackage)?;
    if digest.len() != 64 {
        return Err(PluginManagerError::InvalidPackage);
    }
    Ok(())
}
