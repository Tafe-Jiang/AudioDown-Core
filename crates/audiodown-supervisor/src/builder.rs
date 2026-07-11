use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::trusted_images::{BUILDER_IMAGE, RUNTIME_IMAGE};

pub const BUILD_LOG_LIMIT_BYTES: usize = 1024 * 1024;
pub const BUILD_OUTPUT_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
pub const BUILD_OUTPUT_FILE_LIMIT: usize = 32_768;
pub const BUILD_OUTPUT_FILE_SIZE_LIMIT: u64 = 64 * 1024 * 1024;

const BUILD_PROXY_IMAGE: &str = "audiodown/supervisor:1.0.0-alpha.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPolicy {
    pub image: String,
    pub user: String,
    pub networks: Vec<String>,
    pub network_aliases: Vec<String>,
    pub env: HashMap<String, String>,
    pub command: Vec<String>,
    pub cap_drop: Vec<String>,
    pub security_opt: Vec<String>,
    pub read_only_rootfs: bool,
    pub tmpfs: HashMap<String, String>,
    pub bind_mounts: Vec<String>,
    pub devices: Vec<String>,
    pub privileged: bool,
    pub host_network: bool,
    pub network_disabled: bool,
    pub start_container: bool,
    pub memory_bytes: i64,
    pub nano_cpus: i64,
    pub pids_limit: i64,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPolicy {
    pub name: String,
    pub driver: String,
    pub internal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildNetworks {
    pub internal: NetworkPolicy,
    pub egress: NetworkPolicy,
}

pub fn build_networks(operation_id: &str) -> BuildNetworks {
    BuildNetworks {
        internal: NetworkPolicy {
            name: internal_network(operation_id),
            driver: "bridge".to_string(),
            internal: true,
        },
        egress: NetworkPolicy {
            name: egress_network(operation_id),
            driver: "bridge".to_string(),
            internal: false,
        },
    }
}

pub fn builder_policy(operation_id: &str) -> ContainerPolicy {
    let networks = build_networks(operation_id);
    ContainerPolicy {
        image: BUILDER_IMAGE.to_string(),
        user: "10001:10001".to_string(),
        networks: vec![networks.internal.name],
        network_aliases: Vec::new(),
        env: HashMap::from([
            (
                "HTTPS_PROXY".to_string(),
                "http://audiodown-npm-proxy:18081".to_string(),
            ),
            (
                "HTTP_PROXY".to_string(),
                "http://audiodown-npm-proxy:18081".to_string(),
            ),
            ("NO_PROXY".to_string(), String::new()),
            ("NODE_ENV".to_string(), "production".to_string()),
        ]),
        command: vec![
            "node".to_string(),
            "/opt/audiodown/node22-build-runner.js".to_string(),
        ],
        cap_drop: vec!["ALL".to_string()],
        security_opt: vec!["no-new-privileges:true".to_string()],
        read_only_rootfs: true,
        tmpfs: HashMap::from([(
            "/workspace".to_string(),
            "rw,nosuid,nodev,size=268435456,uid=10001,gid=10001,mode=0700".to_string(),
        )]),
        bind_mounts: Vec::new(),
        devices: Vec::new(),
        privileged: false,
        host_network: false,
        network_disabled: false,
        start_container: true,
        memory_bytes: 512 * 1024 * 1024,
        nano_cpus: 1_000_000_000,
        pids_limit: 128,
        timeout: Duration::from_secs(5 * 60),
    }
}

pub fn proxy_policy(operation_id: &str) -> ContainerPolicy {
    let networks = build_networks(operation_id);
    ContainerPolicy {
        image: BUILD_PROXY_IMAGE.to_string(),
        user: "10002:10002".to_string(),
        networks: vec![networks.internal.name, networks.egress.name],
        network_aliases: vec!["audiodown-npm-proxy".to_string()],
        env: HashMap::new(),
        command: vec![
            "audiodown-supervisor".to_string(),
            "build-proxy".to_string(),
        ],
        cap_drop: vec!["ALL".to_string()],
        security_opt: vec!["no-new-privileges:true".to_string()],
        read_only_rootfs: true,
        tmpfs: HashMap::from([(
            "/tmp".to_string(),
            "rw,noexec,nosuid,nodev,size=16777216,uid=10002,gid=10002,mode=0700".to_string(),
        )]),
        bind_mounts: Vec::new(),
        devices: Vec::new(),
        privileged: false,
        host_network: false,
        network_disabled: false,
        start_container: true,
        memory_bytes: 128 * 1024 * 1024,
        nano_cpus: 500_000_000,
        pids_limit: 64,
        timeout: Duration::from_secs(5 * 60),
    }
}

pub fn assembler_policy() -> ContainerPolicy {
    ContainerPolicy {
        image: RUNTIME_IMAGE.to_string(),
        user: "0:0".to_string(),
        networks: Vec::new(),
        network_aliases: Vec::new(),
        env: HashMap::new(),
        command: Vec::new(),
        cap_drop: vec!["ALL".to_string()],
        security_opt: vec!["no-new-privileges:true".to_string()],
        read_only_rootfs: true,
        tmpfs: HashMap::new(),
        bind_mounts: Vec::new(),
        devices: Vec::new(),
        privileged: false,
        host_network: false,
        network_disabled: true,
        start_container: false,
        memory_bytes: 0,
        nano_cpus: 0,
        pids_limit: 0,
        timeout: Duration::ZERO,
    }
}

fn internal_network(operation_id: &str) -> String {
    format!("audiodown-build-{operation_id}-internal")
}

fn egress_network(operation_id: &str) -> String {
    format!("audiodown-build-{operation_id}-egress")
}

pub fn npm_ci_command(allow_lifecycle_scripts: bool) -> Vec<String> {
    let mut command = vec![
        "npm".to_string(),
        "ci".to_string(),
        "--omit=dev".to_string(),
    ];
    if !allow_lifecycle_scripts {
        command.push("--ignore-scripts".to_string());
    }
    command.extend(["--no-audit".to_string(), "--no-fund".to_string()]);
    command
}

pub fn managed_image_tag(
    plugin_id: &str,
    commit_sha: &str,
    source_hash: &str,
) -> Result<String, BuildPolicyError> {
    if plugin_id.is_empty() || !is_lower_hex(commit_sha, 40) || !is_lower_hex(source_hash, 64) {
        return Err(BuildPolicyError::InvalidImageMetadata);
    }
    let plugin_hash = format!("{:x}", Sha256::digest(plugin_id.as_bytes()));
    Ok(format!(
        "audiodown/plugin-{}:{}-{}",
        &plugin_hash[..12],
        &commit_sha[..12],
        &source_hash[..12]
    ))
}

pub struct BuildConcurrency {
    global: Arc<Semaphore>,
    active_plugins: Arc<Mutex<HashSet<String>>>,
}

impl Default for BuildConcurrency {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildConcurrency {
    pub fn new() -> Self {
        Self {
            global: Arc::new(Semaphore::new(1)),
            active_plugins: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn try_acquire_global(&self) -> Result<OwnedSemaphorePermit, BuildPolicyError> {
        self.global
            .clone()
            .try_acquire_owned()
            .map_err(|_| BuildPolicyError::GlobalBuildBusy)
    }

    pub fn reserve_plugin(&self, plugin_id: &str) -> Result<PluginReservation, BuildPolicyError> {
        let mut active = self
            .active_plugins
            .lock()
            .map_err(|_| BuildPolicyError::ConcurrencyStatePoisoned)?;
        if !active.insert(plugin_id.to_string()) {
            return Err(BuildPolicyError::PluginBuildBusy);
        }
        Ok(PluginReservation {
            plugin_id: plugin_id.to_string(),
            active_plugins: self.active_plugins.clone(),
        })
    }
}

pub struct PluginReservation {
    plugin_id: String,
    active_plugins: Arc<Mutex<HashSet<String>>>,
}

impl Drop for PluginReservation {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active_plugins.lock() {
            active.remove(&self.plugin_id);
        }
    }
}

#[derive(Debug, Default)]
pub struct BuildLog {
    bytes: Vec<u8>,
    terminal: bool,
}

impl BuildLog {
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), BuildPolicyError> {
        if self.terminal {
            return Err(BuildPolicyError::BuildLogLimitExceeded);
        }
        let remaining = BUILD_LOG_LIMIT_BYTES.saturating_sub(self.bytes.len());
        if chunk.len() > remaining {
            self.bytes.extend_from_slice(&chunk[..remaining]);
            self.terminal = true;
            return Err(BuildPolicyError::BuildLogLimitExceeded);
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub fn terminal_code(&self) -> Option<&'static str> {
        self.terminal.then_some("BUILD_LOG_LIMIT_EXCEEDED")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutputEntry {
    pub path: PathBuf,
    pub kind: BuildOutputEntryKind,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutputEntryKind {
    Directory,
    File,
    Symlink { target: PathBuf },
    HardLink { target: PathBuf },
    Device,
    Fifo,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedBuildOutputEntry {
    pub path: PathBuf,
    pub kind: BuildOutputEntryKind,
    pub size: u64,
    pub uid: u64,
    pub gid: u64,
    pub mode: u32,
    pub mtime: u64,
    pub extended_metadata: Vec<(String, Vec<u8>)>,
}

pub fn normalize_build_output(
    entries: Vec<BuildOutputEntry>,
) -> Result<Vec<NormalizedBuildOutputEntry>, BuildPolicyError> {
    if entries.len() > BUILD_OUTPUT_FILE_LIMIT {
        return Err(BuildPolicyError::BuildOutputLimitExceeded);
    }

    let mut seen = HashSet::new();
    let mut total_size = 0_u64;
    let mut normalized = Vec::with_capacity(entries.len());
    for entry in entries {
        validate_relative_path(&entry.path)?;
        if !seen.insert(entry.path.clone()) {
            return Err(BuildPolicyError::UnsafeBuildOutput);
        }

        let mode = match &entry.kind {
            BuildOutputEntryKind::Directory => {
                if entry.size != 0 {
                    return Err(BuildPolicyError::UnsafeBuildOutput);
                }
                0o755
            }
            BuildOutputEntryKind::File => {
                if entry.size > BUILD_OUTPUT_FILE_SIZE_LIMIT {
                    return Err(BuildPolicyError::BuildOutputLimitExceeded);
                }
                total_size = total_size
                    .checked_add(entry.size)
                    .ok_or(BuildPolicyError::BuildOutputLimitExceeded)?;
                if total_size > BUILD_OUTPUT_LIMIT_BYTES {
                    return Err(BuildPolicyError::BuildOutputLimitExceeded);
                }
                0o644
            }
            BuildOutputEntryKind::Symlink { target } => {
                if entry.size != 0 || !symlink_stays_inside(&entry.path, target) {
                    return Err(BuildPolicyError::UnsafeBuildOutput);
                }
                0o777
            }
            BuildOutputEntryKind::HardLink { .. }
            | BuildOutputEntryKind::Device
            | BuildOutputEntryKind::Fifo
            | BuildOutputEntryKind::Other => {
                return Err(BuildPolicyError::UnsafeBuildOutput);
            }
        };

        normalized.push(NormalizedBuildOutputEntry {
            path: entry.path,
            kind: entry.kind,
            size: entry.size,
            uid: 0,
            gid: 0,
            mode,
            mtime: 0,
            extended_metadata: Vec::new(),
        });
    }
    normalized.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(normalized)
}

fn validate_relative_path(path: &Path) -> Result<(), BuildPolicyError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.to_string_lossy().contains('\\')
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
                    | Component::CurDir
            )
        })
    {
        return Err(BuildPolicyError::UnsafeBuildOutput);
    }
    Ok(())
}

fn symlink_stays_inside(link_path: &Path, target: &Path) -> bool {
    if target.as_os_str().is_empty()
        || target.is_absolute()
        || target.to_string_lossy().contains('\\')
    {
        return false;
    }

    let mut depth = link_path
        .parent()
        .map(|parent| parent.components().count())
        .unwrap_or(0);
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    depth > 0
}

pub struct FinalImageMetadata<'a> {
    pub installation_id: &'a str,
    pub plugin_id: &'a str,
    pub commit_sha: &'a str,
    pub source_hash: &'a str,
    pub manifest_hash: &'a str,
    pub base_image_digest: &'a str,
    pub sdk_hash: &'a str,
}

pub fn final_image_labels(
    metadata: FinalImageMetadata<'_>,
) -> Result<HashMap<String, String>, BuildPolicyError> {
    if metadata.installation_id.is_empty()
        || metadata.plugin_id.is_empty()
        || !is_lower_hex(metadata.commit_sha, 40)
        || !is_lower_hex(metadata.source_hash, 64)
        || !is_lower_hex(metadata.manifest_hash, 64)
        || !metadata
            .base_image_digest
            .strip_prefix("sha256:")
            .is_some_and(|digest| is_lower_hex(digest, 64))
        || !is_lower_hex(metadata.sdk_hash, 64)
    {
        return Err(BuildPolicyError::InvalidImageMetadata);
    }

    Ok(HashMap::from([
        ("io.audiodown.managed".to_string(), "true".to_string()),
        (
            "io.audiodown.installation".to_string(),
            metadata.installation_id.to_string(),
        ),
        (
            "io.audiodown.plugin-id".to_string(),
            metadata.plugin_id.to_string(),
        ),
        (
            "io.audiodown.commit-sha".to_string(),
            metadata.commit_sha.to_string(),
        ),
        (
            "io.audiodown.source-hash".to_string(),
            metadata.source_hash.to_string(),
        ),
        (
            "io.audiodown.manifest-hash".to_string(),
            metadata.manifest_hash.to_string(),
        ),
        (
            "io.audiodown.base-image-digest".to_string(),
            metadata.base_image_digest.to_string(),
        ),
        (
            "io.audiodown.sdk-hash".to_string(),
            metadata.sdk_hash.to_string(),
        ),
    ]))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BuildPolicyError {
    #[error("another plugin build holds the global build permit")]
    GlobalBuildBusy,
    #[error("the plugin already has an active build operation")]
    PluginBuildBusy,
    #[error("build concurrency state is unavailable")]
    ConcurrencyStatePoisoned,
    #[error("BUILD_LOG_LIMIT_EXCEEDED")]
    BuildLogLimitExceeded,
    #[error("build output violates archive safety policy")]
    UnsafeBuildOutput,
    #[error("build output exceeds a fixed archive limit")]
    BuildOutputLimitExceeded,
    #[error("managed image metadata is invalid")]
    InvalidImageMetadata,
}
