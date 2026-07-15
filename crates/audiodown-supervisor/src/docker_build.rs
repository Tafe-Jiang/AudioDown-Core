use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use async_trait::async_trait;
use audiodown_supervisor_protocol::{PluginBuildLog, PluginBuildLogStream, PluginInstallArtifact};
use bollard::{
    container::LogOutput,
    exec::{StartExecOptions, StartExecResults},
    models::{
        ContainerConfig, ContainerCreateBody, EndpointSettings, ExecConfig, HostConfig,
        NetworkConnectRequest, NetworkCreateRequest, NetworkingConfig,
    },
    query_parameters::{
        BuildImageOptionsBuilder, CommitContainerOptionsBuilder, CreateContainerOptionsBuilder,
        LogsOptionsBuilder, StopContainerOptionsBuilder, UploadToContainerOptionsBuilder,
    },
    Docker,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, EntryType, Header};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{
    builder::{
        assembler_policy, builder_policy, final_image_labels, managed_image_tag, proxy_policy,
        BuildConcurrency, BuildOutputEntry, BuildOutputEntryKind, FinalImageMetadata,
        BUILD_LOG_LIMIT_BYTES, BUILD_OUTPUT_FILE_LIMIT, BUILD_OUTPUT_FILE_SIZE_LIMIT,
        BUILD_OUTPUT_LIMIT_BYTES,
    },
    install_operation::{BuildAdapterError, BuildOutput, BuildRequest, InstallBuildAdapter},
    prepared_install::validate_prepared_install,
    trusted_images::{
        pinned_base_reference, trusted_image_labels, verify_repo_digests,
        verify_trusted_image_labels, NodeImageLock, TrustedImageKind, BUILDER_IMAGE,
        POLICY_VERSION, RUNTIME_IMAGE,
    },
};

const SUPERVISOR_IMAGE: &str = "audiodown/supervisor:1.0.0-alpha.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationResourceNames {
    pub internal_network: String,
    pub egress_network: String,
    pub proxy_container: String,
    pub builder_container: String,
    pub assembler_container: String,
}

pub fn operation_resource_names(operation_id: Uuid) -> OperationResourceNames {
    OperationResourceNames {
        internal_network: format!("audiodown-build-{operation_id}-internal"),
        egress_network: format!("audiodown-build-{operation_id}-egress"),
        proxy_container: format!("audiodown-build-{operation_id}-proxy"),
        builder_container: format!("audiodown-build-{operation_id}-builder"),
        assembler_container: format!("audiodown-build-{operation_id}-assembler"),
    }
}

fn operation_resource_labels(operation_id: Uuid, role: &str) -> HashMap<String, String> {
    HashMap::from([
        ("io.audiodown.managed".to_string(), "true".to_string()),
        ("io.audiodown.resource-role".to_string(), role.to_string()),
        (
            "io.audiodown.operation-id".to_string(),
            operation_id.to_string(),
        ),
    ])
}

pub fn builder_container_config(
    operation_id: Uuid,
    allow_lifecycle_scripts: bool,
) -> ContainerCreateBody {
    let policy = builder_policy(&operation_id.to_string());
    let host_config = host_config(&policy);
    let env = vec![
        format!("AUDIODOWN_ALLOW_LIFECYCLE_SCRIPTS={allow_lifecycle_scripts}"),
        "HTTP_PROXY=http://audiodown-npm-proxy:18081".to_string(),
        "HTTPS_PROXY=http://audiodown-npm-proxy:18081".to_string(),
        "NODE_ENV=production".to_string(),
        "NO_PROXY=".to_string(),
    ];
    ContainerCreateBody {
        image: Some(policy.image),
        user: Some(policy.user),
        working_dir: Some("/workspace".to_string()),
        env: Some(env),
        entrypoint: Some(policy.command),
        labels: Some(operation_resource_labels(operation_id, "plugin-build")),
        network_disabled: Some(false),
        host_config: Some(host_config),
        ..Default::default()
    }
}

pub fn proxy_container_config(operation_id: Uuid) -> ContainerCreateBody {
    let policy = proxy_policy(&operation_id.to_string());
    let host_config = host_config(&policy);
    let names = operation_resource_names(operation_id);
    ContainerCreateBody {
        image: Some(SUPERVISOR_IMAGE.to_string()),
        user: Some(policy.user),
        entrypoint: Some(policy.command),
        labels: Some(operation_resource_labels(
            operation_id,
            "plugin-build-proxy",
        )),
        network_disabled: Some(false),
        host_config: Some(host_config),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                names.internal_network,
                EndpointSettings {
                    aliases: Some(vec!["audiodown-npm-proxy".to_string()]),
                    ..Default::default()
                },
            )])),
        }),
        ..Default::default()
    }
}

pub fn assembler_container_config(operation_id: Uuid) -> ContainerCreateBody {
    let _ = operation_id;
    let policy = assembler_policy();
    let host_config = host_config(&policy);
    ContainerCreateBody {
        image: Some(policy.image),
        user: Some(policy.user),
        network_disabled: Some(true),
        host_config: Some(host_config),
        ..Default::default()
    }
}

fn host_config(policy: &crate::builder::ContainerPolicy) -> HostConfig {
    HostConfig {
        binds: Some(policy.bind_mounts.clone()),
        devices: Some(Vec::new()),
        network_mode: Some(if policy.network_disabled {
            "none".to_string()
        } else {
            policy.networks.first().cloned().unwrap_or_default()
        }),
        privileged: Some(policy.privileged),
        readonly_rootfs: Some(policy.read_only_rootfs),
        cap_drop: Some(policy.cap_drop.clone()),
        security_opt: Some(policy.security_opt.clone()),
        tmpfs: Some(policy.tmpfs.clone()),
        memory: Some(policy.memory_bytes),
        memory_swap: Some(policy.memory_bytes),
        nano_cpus: Some(policy.nano_cpus),
        pids_limit: Some(policy.pids_limit),
        init: Some(true),
        auto_remove: Some(false),
        ..Default::default()
    }
}

pub fn build_source_archive(root: &Path) -> Result<Vec<u8>, DockerBuildError> {
    require_directory(root)?;
    let mut paths = Vec::new();
    collect_source_entries(root, root, &mut paths)?;
    paths.sort_by(|left, right| left.0.cmp(&right.0));

    let mut output = Vec::new();
    {
        let mut builder = Builder::new(&mut output);
        append_directory(&mut builder, "input/", 10001, 10001)?;
        for (relative, absolute, directory) in paths {
            let archive_path = format!("input/{relative}");
            if directory {
                append_directory(&mut builder, &format!("{archive_path}/"), 10001, 10001)?;
            } else {
                let bytes = fs::read(&absolute).map_err(|_| DockerBuildError::UnsafeBuildInput)?;
                append_file(&mut builder, &archive_path, &bytes, 0o644, 10001, 10001)?;
            }
        }
        append_file(
            &mut builder,
            "input/.input-ready",
            b"ready\n",
            0o600,
            10001,
            10001,
        )?;
        builder.finish().map_err(|_| DockerBuildError::ArchiveIo)?;
    }
    Ok(output)
}

fn collect_source_entries(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<(String, PathBuf, bool)>,
) -> Result<(), DockerBuildError> {
    for entry in fs::read_dir(directory).map_err(|_| DockerBuildError::UnsafeBuildInput)? {
        let entry = entry.map_err(|_| DockerBuildError::UnsafeBuildInput)?;
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|_| DockerBuildError::UnsafeBuildInput)?;
        if metadata.file_type().is_symlink() {
            return Err(DockerBuildError::UnsafeBuildInput);
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| DockerBuildError::UnsafeBuildInput)?
            .to_str()
            .ok_or(DockerBuildError::UnsafeBuildInput)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if metadata.is_dir() {
            entries.push((relative, entry.path(), true));
            collect_source_entries(root, &entry.path(), entries)?;
        } else if metadata.is_file() {
            entries.push((relative, entry.path(), false));
        } else {
            return Err(DockerBuildError::UnsafeBuildInput);
        }
    }
    Ok(())
}

pub fn normalize_output_archive(input: &[u8]) -> Result<Vec<u8>, DockerBuildError> {
    if input.len() as u64 > BUILD_OUTPUT_LIMIT_BYTES {
        return Err(DockerBuildError::BuildOutputLimitExceeded);
    }
    let mut archive = Archive::new(Cursor::new(input));
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    let mut total_size = 0_u64;
    let mut count = 0_usize;
    let mut entries = Vec::new();

    for entry in archive
        .entries()
        .map_err(|_| DockerBuildError::UnsafeBuildOutput)?
    {
        let mut entry = entry.map_err(|_| DockerBuildError::UnsafeBuildOutput)?;
        count += 1;
        if count > BUILD_OUTPUT_FILE_LIMIT {
            return Err(DockerBuildError::BuildOutputLimitExceeded);
        }
        let path = entry
            .path()
            .map_err(|_| DockerBuildError::UnsafeBuildOutput)?
            .into_owned();
        let relative = strip_output_prefix(&path)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        validate_relative_path(&relative)?;
        if !seen.insert(relative.clone()) {
            return Err(DockerBuildError::UnsafeBuildOutput);
        }
        let entry_type = entry.header().entry_type();
        let size = entry
            .header()
            .size()
            .map_err(|_| DockerBuildError::UnsafeBuildOutput)?;
        let (kind, contents) = if entry_type.is_dir() {
            (BuildOutputEntryKind::Directory, Vec::new())
        } else if entry_type.is_file() {
            if size > BUILD_OUTPUT_FILE_SIZE_LIMIT {
                return Err(DockerBuildError::BuildOutputLimitExceeded);
            }
            total_size = total_size
                .checked_add(size)
                .ok_or(DockerBuildError::BuildOutputLimitExceeded)?;
            if total_size > BUILD_OUTPUT_LIMIT_BYTES {
                return Err(DockerBuildError::BuildOutputLimitExceeded);
            }
            let mut contents = Vec::with_capacity(size as usize);
            entry
                .read_to_end(&mut contents)
                .map_err(|_| DockerBuildError::UnsafeBuildOutput)?;
            (BuildOutputEntryKind::File, contents)
        } else if entry_type.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|_| DockerBuildError::UnsafeBuildOutput)?
                .ok_or(DockerBuildError::UnsafeBuildOutput)?
                .into_owned();
            (BuildOutputEntryKind::Symlink { target }, Vec::new())
        } else {
            return Err(DockerBuildError::UnsafeBuildOutput);
        };
        crate::builder::normalize_build_output(vec![BuildOutputEntry {
            path: relative.clone(),
            kind: kind.clone(),
            size,
        }])
        .map_err(|_| DockerBuildError::UnsafeBuildOutput)?;
        entries.push((relative, kind, contents));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    {
        let mut builder = Builder::new(&mut normalized);
        for (path, kind, contents) in entries {
            let path_text = path.to_str().ok_or(DockerBuildError::UnsafeBuildOutput)?;
            match kind {
                BuildOutputEntryKind::Directory => {
                    append_directory(&mut builder, &format!("{path_text}/"), 0, 0)?
                }
                BuildOutputEntryKind::File => {
                    append_file(&mut builder, path_text, &contents, 0o644, 0, 0)?
                }
                BuildOutputEntryKind::Symlink { target } => {
                    append_symlink(&mut builder, path_text, &target)?
                }
                _ => return Err(DockerBuildError::UnsafeBuildOutput),
            }
        }
        builder.finish().map_err(|_| DockerBuildError::ArchiveIo)?;
    }
    Ok(normalized)
}

pub fn complete_tar_length(bytes: &[u8]) -> Option<usize> {
    const BLOCK: usize = 512;

    let mut offset = 0_usize;
    loop {
        let header_end = offset.checked_add(BLOCK)?;
        let header = bytes.get(offset..header_end)?;
        if header.iter().all(|byte| *byte == 0) {
            let terminator_end = header_end.checked_add(BLOCK)?;
            let second = bytes.get(header_end..terminator_end)?;
            return second
                .iter()
                .all(|byte| *byte == 0)
                .then_some(terminator_end);
        }

        let size_field = header.get(124..136)?;
        let size_text = std::str::from_utf8(size_field)
            .ok()?
            .trim_matches(['\0', ' ']);
        let size = if size_text.is_empty() {
            0_usize
        } else {
            usize::from_str_radix(size_text, 8).ok()?
        };
        let padded_size = size
            .checked_add(BLOCK - 1)?
            .checked_div(BLOCK)?
            .checked_mul(BLOCK)?;
        offset = header_end.checked_add(padded_size)?;
    }
}

fn strip_output_prefix(path: &Path) -> Result<PathBuf, DockerBuildError> {
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(value)) if value == "output" => {}
        _ => return Err(DockerBuildError::UnsafeBuildOutput),
    }
    Ok(components.collect())
}

fn validate_relative_path(path: &Path) -> Result<(), DockerBuildError> {
    if path.is_absolute()
        || path.as_os_str().is_empty()
        || path.to_string_lossy().contains('\\')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(DockerBuildError::UnsafeBuildOutput);
    }
    Ok(())
}

pub struct TrustedImageInputs {
    pub pinned_base_reference: String,
    pub base_image_digest: String,
    pub sdk_hash: String,
    pub builder_asset_hash: String,
    pub runtime_asset_hash: String,
    pub builder_context: Vec<u8>,
    pub runtime_context: Vec<u8>,
}

pub fn trusted_image_inputs() -> Result<TrustedImageInputs, DockerBuildError> {
    let lock = NodeImageLock::embedded().map_err(|_| DockerBuildError::TrustedImage)?;
    let pinned_base_reference =
        pinned_base_reference(&lock).map_err(|_| DockerBuildError::TrustedImage)?;
    let sdk_files = embedded_sdk_files();
    let sdk_hash = hash_embedded_files(&sdk_files);
    let builder_context = trusted_context(&[
        (
            "Dockerfile",
            include_bytes!("../../../docker/plugin-runtime/node22-builder.Dockerfile"),
        ),
        (
            "docker/plugin-runtime/node22-build-runner.js",
            include_bytes!("../../../docker/plugin-runtime/node22-build-runner.js"),
        ),
    ])?;
    let runtime_context = runtime_context(&sdk_files)?;
    let builder_asset_hash = hash_bytes(&builder_context);
    let runtime_asset_hash = hash_bytes(&runtime_context);
    Ok(TrustedImageInputs {
        pinned_base_reference,
        base_image_digest: lock.digest,
        sdk_hash,
        builder_asset_hash,
        runtime_asset_hash,
        builder_context,
        runtime_context,
    })
}

fn trusted_context(files: &[(&str, &[u8])]) -> Result<Vec<u8>, DockerBuildError> {
    let mut output = Vec::new();
    {
        let mut builder = Builder::new(&mut output);
        for (path, contents) in files {
            append_file(&mut builder, path, contents, 0o644, 0, 0)?;
        }
        builder.finish().map_err(|_| DockerBuildError::ArchiveIo)?;
    }
    Ok(output)
}

fn runtime_context(sdk_files: &[(&str, &[u8])]) -> Result<Vec<u8>, DockerBuildError> {
    let mut output = Vec::new();
    {
        let mut builder = Builder::new(&mut output);
        append_file(
            &mut builder,
            "Dockerfile",
            include_bytes!("../../../docker/plugin-runtime/node22-runtime.Dockerfile"),
            0o644,
            0,
            0,
        )?;
        append_directory(&mut builder, "docker/", 0, 0)?;
        append_directory(&mut builder, "docker/plugin-runtime/", 0, 0)?;
        append_file(
            &mut builder,
            "docker/plugin-runtime/plugin-token-bootstrap.sh",
            include_bytes!("../../../docker/plugin-runtime/plugin-token-bootstrap.sh"),
            0o555,
            0,
            0,
        )?;
        append_directory(&mut builder, "plugin-sdk/", 0, 0)?;
        append_directory(&mut builder, "plugin-sdk/node/", 0, 0)?;
        append_directory(&mut builder, "plugin-sdk/node/src/", 0, 0)?;
        append_directory(&mut builder, "plugin-sdk/node/test/", 0, 0)?;
        for (relative, contents) in sdk_files {
            append_file(
                &mut builder,
                &format!("plugin-sdk/node/{relative}"),
                contents,
                0o644,
                0,
                0,
            )?;
        }
        builder.finish().map_err(|_| DockerBuildError::ArchiveIo)?;
    }
    Ok(output)
}

fn hash_embedded_files(files: &[(&str, &[u8])]) -> String {
    let mut hasher = Sha256::new();
    for (relative, contents) in files {
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((contents.len() as u64).to_be_bytes());
        hasher.update(contents);
    }
    format!("{:x}", hasher.finalize())
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn embedded_sdk_files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        (
            "package-lock.json",
            include_bytes!("../../../plugin-sdk/node/package-lock.json"),
        ),
        (
            "package.json",
            include_bytes!("../../../plugin-sdk/node/package.json"),
        ),
        (
            "src/content.js",
            include_bytes!("../../../plugin-sdk/node/src/content.js"),
        ),
        (
            "src/index.js",
            include_bytes!("../../../plugin-sdk/node/src/index.js"),
        ),
        (
            "src/logger.js",
            include_bytes!("../../../plugin-sdk/node/src/logger.js"),
        ),
        (
            "src/rpc.js",
            include_bytes!("../../../plugin-sdk/node/src/rpc.js"),
        ),
        (
            "test/content.test.js",
            include_bytes!("../../../plugin-sdk/node/test/content.test.js"),
        ),
        (
            "test/sdk.test.js",
            include_bytes!("../../../plugin-sdk/node/test/sdk.test.js"),
        ),
    ]
}

pub struct ManagedImageInput<'a> {
    pub installation_id: &'a str,
    pub operation_id: Uuid,
    pub plugin_id: &'a str,
    pub commit_sha: &'a str,
    pub source_hash: &'a str,
    pub manifest_hash: &'a str,
    pub base_image_digest: &'a str,
    pub sdk_hash: &'a str,
}

pub struct ManagedImagePlan {
    pub tag: String,
    pub labels: HashMap<String, String>,
}

pub fn managed_image_plan(
    input: ManagedImageInput<'_>,
) -> Result<ManagedImagePlan, DockerBuildError> {
    let tag = managed_image_tag(input.plugin_id, input.commit_sha, input.source_hash)
        .map_err(|_| DockerBuildError::InvalidImageMetadata)?;
    let mut labels = final_image_labels(FinalImageMetadata {
        installation_id: input.installation_id,
        plugin_id: input.plugin_id,
        commit_sha: input.commit_sha,
        source_hash: input.source_hash,
        manifest_hash: input.manifest_hash,
        base_image_digest: input.base_image_digest,
        sdk_hash: input.sdk_hash,
    })
    .map_err(|_| DockerBuildError::InvalidImageMetadata)?;
    labels.insert(
        "io.audiodown.operation-id".to_string(),
        input.operation_id.to_string(),
    );
    Ok(ManagedImagePlan { tag, labels })
}

fn append_directory(
    builder: &mut Builder<&mut Vec<u8>>,
    path: &str,
    uid: u64,
    gid: u64,
) -> Result<(), DockerBuildError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Directory);
    header.set_size(0);
    header.set_mode(0o755);
    header.set_uid(uid);
    header.set_gid(gid);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(Vec::<u8>::new()))
        .map_err(|_| DockerBuildError::ArchiveIo)
}

fn append_file(
    builder: &mut Builder<&mut Vec<u8>>,
    path: &str,
    contents: &[u8],
    mode: u32,
    uid: u64,
    gid: u64,
) -> Result<(), DockerBuildError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(contents.len() as u64);
    header.set_mode(mode);
    header.set_uid(uid);
    header.set_gid(gid);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(contents))
        .map_err(|_| DockerBuildError::ArchiveIo)
}

fn append_symlink(
    builder: &mut Builder<&mut Vec<u8>>,
    path: &str,
    target: &Path,
) -> Result<(), DockerBuildError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header
        .set_link_name(target)
        .map_err(|_| DockerBuildError::UnsafeBuildOutput)?;
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(Vec::<u8>::new()))
        .map_err(|_| DockerBuildError::ArchiveIo)
}

fn require_directory(path: &Path) -> Result<(), DockerBuildError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| DockerBuildError::UnsafeBuildInput)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DockerBuildError::UnsafeBuildInput);
    }
    Ok(())
}

pub struct DockerBuildAdapter {
    docker: Docker,
    plugin_data: PathBuf,
    installation_id: String,
    concurrency: BuildConcurrency,
}

impl DockerBuildAdapter {
    pub fn connect(
        plugin_data: PathBuf,
        installation_id: String,
    ) -> Result<Self, bollard::errors::Error> {
        Ok(Self {
            docker: Docker::connect_with_local_defaults()?,
            plugin_data,
            installation_id,
            concurrency: BuildConcurrency::new(),
        })
    }

    async fn execute_build(
        &self,
        request: &BuildRequest,
    ) -> Result<BuildOutput, BuildAdapterError> {
        let prepared =
            validate_prepared_install(&self.plugin_data, &request.plugin_id, request.operation_id)
                .await
                .map_err(|_| BuildAdapterError::new("INVALID_PREPARED_INSTALL"))?;
        let _plugin = self
            .concurrency
            .reserve_plugin(request.plugin_id.as_str())
            .map_err(|_| BuildAdapterError::new("BUILD_BUSY"))?;
        let _global = self
            .concurrency
            .try_acquire_global()
            .map_err(|_| BuildAdapterError::new("BUILD_BUSY"))?;
        let source = build_source_archive(&prepared.plugin_root)
            .map_err(|error| BuildAdapterError::new(error.code()))?;
        let trusted =
            trusted_image_inputs().map_err(|error| BuildAdapterError::new(error.code()))?;
        self.ensure_trusted_images(&trusted).await?;

        let plan = managed_image_plan(ManagedImageInput {
            installation_id: &request.installation_id,
            operation_id: request.operation_id,
            plugin_id: request.plugin_id.as_str(),
            commit_sha: &prepared.commit_sha,
            source_hash: &prepared.source_hash,
            manifest_hash: &prepared.manifest_hash,
            base_image_digest: &trusted.base_image_digest,
            sdk_hash: &trusted.sdk_hash,
        })
        .map_err(|error| BuildAdapterError::new(error.code()))?;
        let names = operation_resource_names(request.operation_id);
        self.create_build_networks(&names).await?;

        let proxy = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&names.proxy_container)
                        .build(),
                ),
                proxy_container_config(request.operation_id),
            )
            .await
            .map_err(|_| BuildAdapterError::new("PROXY_CREATE_FAILED"))?;
        self.docker
            .connect_network(
                &names.egress_network,
                NetworkConnectRequest {
                    container: Some(proxy.id.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| BuildAdapterError::new("PROXY_NETWORK_FAILED"))?;

        let builder = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&names.builder_container)
                        .build(),
                ),
                builder_container_config(request.operation_id, prepared.allow_lifecycle_scripts),
            )
            .await
            .map_err(|_| BuildAdapterError::new("BUILDER_CREATE_FAILED"))?;
        self.docker
            .start_container(
                &proxy.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|_| BuildAdapterError::new("PROXY_START_FAILED"))?;
        self.docker
            .start_container(
                &builder.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|_| BuildAdapterError::new("BUILDER_START_FAILED"))?;
        self.upload_source(&builder.id, &source).await?;

        let status_result = self.wait_for_build_status(&builder.id).await;
        let build_logs = self.collect_build_logs(&builder.id).await?;
        let status = status_result
            .map_err(|error| BuildAdapterError::with_logs(error.code(), build_logs.clone()))?;
        if status.state != "completed" {
            return Err(BuildAdapterError::with_logs(
                status.code.as_deref().unwrap_or("NPM_CI_FAILED"),
                build_logs,
            ));
        }

        let output_archive = self.download_output(&builder.id).await?;
        let normalized = normalize_output_archive(&output_archive)
            .map_err(|error| BuildAdapterError::with_logs(error.code(), build_logs.clone()))?;
        let assembler = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&names.assembler_container)
                        .build(),
                ),
                assembler_container_config(request.operation_id),
            )
            .await
            .map_err(|_| {
                BuildAdapterError::with_logs("ASSEMBLER_CREATE_FAILED", build_logs.clone())
            })?;
        self.docker
            .upload_to_container(
                &assembler.id,
                Some(
                    UploadToContainerOptionsBuilder::new()
                        .path("/plugin")
                        .build(),
                ),
                bollard::body_full(normalized.into()),
            )
            .await
            .map_err(|_| {
                BuildAdapterError::with_logs("ASSEMBLER_UPLOAD_FAILED", build_logs.clone())
            })?;

        let (repository, tag) = plan
            .tag
            .rsplit_once(':')
            .ok_or_else(|| BuildAdapterError::new("INVALID_IMAGE_METADATA"))?;
        let committed = self
            .docker
            .commit_container(
                CommitContainerOptionsBuilder::new()
                    .container(&assembler.id)
                    .repo(repository)
                    .tag(tag)
                    .pause(true)
                    .build(),
                ContainerConfig {
                    user: Some("10002:10002".to_string()),
                    working_dir: Some("/plugin".to_string()),
                    env: Some(vec![
                        "AUDIODOWN_NODE_SDK_PATH=/sdk/src/index.js".to_string(),
                        "NODE_ENV=production".to_string(),
                    ]),
                    labels: Some(plan.labels.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| BuildAdapterError::with_logs("IMAGE_COMMIT_FAILED", build_logs.clone()))?;
        let image_id = committed.id;
        if image_id.is_empty() {
            return Err(BuildAdapterError::new("IMAGE_COMMIT_FAILED"));
        }
        self.verify_managed_image(&image_id, &plan.labels).await?;

        let manifest = fs::read(prepared.plugin_root.join("audiodown-plugin.json"))
            .map_err(|_| BuildAdapterError::new("MANIFEST_READ_FAILED"))?;
        Ok(BuildOutput {
            artifact: PluginInstallArtifact {
                image_id,
                repository_id: prepared.repository_id,
                commit_sha: prepared.commit_sha,
                source_hash: prepared.source_hash,
                manifest_hash: prepared.manifest_hash,
            },
            manifest,
            base_image_digest: trusted.base_image_digest,
            sdk_hash: trusted.sdk_hash,
            build_logs,
        })
    }

    async fn upload_source(
        &self,
        builder_id: &str,
        source: &[u8],
    ) -> Result<(), BuildAdapterError> {
        let exec = self
            .docker
            .create_exec(
                builder_id,
                ExecConfig {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        concat!(
                            "head -c \"$1\" > /workspace/source.tar",
                            " && tar -xf /workspace/source.tar -C /workspace",
                            " && rm -f /workspace/source.tar"
                        )
                        .to_string(),
                        "audiodown-upload".to_string(),
                        source.len().to_string(),
                    ]),
                    user: Some("10001:10001".to_string()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| BuildAdapterError::new("BUILD_INPUT_UPLOAD_FAILED"))?;
        let StartExecResults::Attached { output, mut input } = self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|_| BuildAdapterError::new("BUILD_INPUT_UPLOAD_FAILED"))?
        else {
            return Err(BuildAdapterError::new("BUILD_INPUT_UPLOAD_FAILED"));
        };
        input
            .write_all(source)
            .await
            .map_err(|_| BuildAdapterError::new("BUILD_INPUT_UPLOAD_FAILED"))?;
        drop(input);
        drop(output);
        for _ in 0..100 {
            let inspected = self
                .docker
                .inspect_exec(&exec.id)
                .await
                .map_err(|_| BuildAdapterError::new("BUILD_INPUT_UPLOAD_FAILED"))?;
            if inspected.running == Some(false) {
                return if inspected.exit_code == Some(0) {
                    Ok(())
                } else {
                    Err(BuildAdapterError::new("BUILD_INPUT_UPLOAD_FAILED"))
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Err(BuildAdapterError::new("BUILD_INPUT_UPLOAD_FAILED"))
    }

    async fn ensure_trusted_images(
        &self,
        trusted: &TrustedImageInputs,
    ) -> Result<(), BuildAdapterError> {
        for (kind, image, context) in [
            (
                TrustedImageKind::Builder,
                BUILDER_IMAGE,
                trusted.builder_context.as_slice(),
            ),
            (
                TrustedImageKind::Runtime,
                RUNTIME_IMAGE,
                trusted.runtime_context.as_slice(),
            ),
        ] {
            let asset_hash = match kind {
                TrustedImageKind::Builder => &trusted.builder_asset_hash,
                TrustedImageKind::Runtime => &trusted.runtime_asset_hash,
            };
            let expected = trusted_image_labels(
                kind,
                &trusted.base_image_digest,
                &trusted.sdk_hash,
                asset_hash,
            );
            let valid = match self.docker.inspect_image(image).await {
                Ok(inspect) => {
                    inspect
                        .config
                        .and_then(|config| config.labels)
                        .is_some_and(|labels| {
                            verify_trusted_image_labels(
                                kind,
                                &trusted.base_image_digest,
                                &trusted.sdk_hash,
                                asset_hash,
                                &labels,
                            )
                            .is_ok()
                        })
                }
                Err(_) => false,
            };
            if valid {
                continue;
            }
            let args = HashMap::from([
                (
                    "BASE_IMAGE_DIGEST".to_string(),
                    trusted.base_image_digest.clone(),
                ),
                ("SDK_HASH".to_string(), trusted.sdk_hash.clone()),
                ("POLICY_VERSION".to_string(), POLICY_VERSION.to_string()),
            ]);
            let options = BuildImageOptionsBuilder::new()
                .dockerfile("Dockerfile")
                .t(image)
                .pull("true")
                .nocache(true)
                .rm(true)
                .forcerm(true)
                .networkmode("none")
                .buildargs(&args)
                .labels(&expected)
                .build();
            let mut stream = self.docker.build_image(
                options,
                None,
                Some(bollard::body_full(context.to_vec().into())),
            );
            while let Some(item) = stream.next().await {
                let info =
                    item.map_err(|_| BuildAdapterError::new("TRUSTED_IMAGE_BUILD_FAILED"))?;
                if info.error.is_some() || info.error_detail.is_some() {
                    return Err(BuildAdapterError::new("TRUSTED_IMAGE_BUILD_FAILED"));
                }
            }
            let inspect = self
                .docker
                .inspect_image(image)
                .await
                .map_err(|_| BuildAdapterError::new("TRUSTED_IMAGE_BUILD_FAILED"))?;
            let labels = inspect
                .config
                .and_then(|config| config.labels)
                .ok_or_else(|| BuildAdapterError::new("TRUSTED_IMAGE_ATTESTATION_FAILED"))?;
            verify_trusted_image_labels(
                kind,
                &trusted.base_image_digest,
                &trusted.sdk_hash,
                asset_hash,
                &labels,
            )
            .map_err(|_| BuildAdapterError::new("TRUSTED_IMAGE_ATTESTATION_FAILED"))?;
        }
        let base = self
            .docker
            .inspect_image(&trusted.pinned_base_reference)
            .await
            .map_err(|_| BuildAdapterError::new("BASE_IMAGE_DIGEST_FAILED"))?;
        verify_repo_digests(
            &NodeImageLock {
                image: "node:22-bookworm-slim".to_string(),
                digest: trusted.base_image_digest.clone(),
            },
            &base.repo_digests.unwrap_or_default(),
        )
        .map_err(|_| BuildAdapterError::new("BASE_IMAGE_DIGEST_FAILED"))
    }

    async fn create_build_networks(
        &self,
        names: &OperationResourceNames,
    ) -> Result<(), BuildAdapterError> {
        self.docker
            .create_network(NetworkCreateRequest {
                name: names.internal_network.clone(),
                driver: Some("bridge".to_string()),
                internal: Some(true),
                attachable: Some(false),
                ..Default::default()
            })
            .await
            .map_err(|_| BuildAdapterError::new("BUILD_NETWORK_CREATE_FAILED"))?;
        self.docker
            .create_network(NetworkCreateRequest {
                name: names.egress_network.clone(),
                driver: Some("bridge".to_string()),
                internal: Some(false),
                attachable: Some(false),
                ..Default::default()
            })
            .await
            .map_err(|_| BuildAdapterError::new("BUILD_NETWORK_CREATE_FAILED"))?;
        Ok(())
    }

    async fn wait_for_build_status(
        &self,
        builder_id: &str,
    ) -> Result<BuildStatus, DockerBuildError> {
        let exec = self
            .docker
            .create_exec(
                builder_id,
                ExecConfig {
                    attach_stdout: Some(false),
                    attach_stderr: Some(false),
                    cmd: Some(vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        concat!(
                            "attempt=0;",
                            " while [ \"$attempt\" -lt 3000 ]; do",
                            " test -f /workspace/status.json && exit 0;",
                            " attempt=$((attempt + 1)); sleep 0.1;",
                            " done;",
                            " exit 1"
                        )
                        .to_string(),
                    ]),
                    user: Some("10001:10001".to_string()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| DockerBuildError::ArchiveIo)?;
        let StartExecResults::Detached = self
            .docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(|_| DockerBuildError::ArchiveIo)?
        else {
            return Err(DockerBuildError::ArchiveIo);
        };
        let wait = async {
            loop {
                let inspected = self
                    .docker
                    .inspect_exec(&exec.id)
                    .await
                    .map_err(|_| DockerBuildError::ArchiveIo)?;
                if inspected.running == Some(false) {
                    return if inspected.exit_code == Some(0) {
                        Ok(())
                    } else {
                        Err(DockerBuildError::BuildTimeout)
                    };
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5 * 60), wait)
            .await
            .map_err(|_| DockerBuildError::BuildTimeout)??;

        let read_status = async {
            let bytes = self
                .download_exec_archive(builder_id, "status.json", 1024 * 1024)
                .await?;
            let mut archive = Archive::new(Cursor::new(bytes));
            let mut entries = archive.entries().map_err(|_| DockerBuildError::ArchiveIo)?;
            let mut entry = entries
                .next()
                .ok_or(DockerBuildError::ArchiveIo)?
                .map_err(|_| DockerBuildError::ArchiveIo)?;
            let mut json = Vec::new();
            entry
                .read_to_end(&mut json)
                .map_err(|_| DockerBuildError::ArchiveIo)?;
            serde_json::from_slice(&json).map_err(|_| DockerBuildError::ArchiveIo)
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), read_status)
            .await
            .map_err(|_| DockerBuildError::BuildTimeout)?
    }

    async fn download_exec_archive(
        &self,
        container_id: &str,
        entry: &str,
        limit: usize,
    ) -> Result<Vec<u8>, DockerBuildError> {
        let exec = self
            .docker
            .create_exec(
                container_id,
                ExecConfig {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(vec![
                        "tar".to_string(),
                        "-cf".to_string(),
                        "-".to_string(),
                        "-C".to_string(),
                        "/workspace".to_string(),
                        entry.to_string(),
                    ]),
                    user: Some("10001:10001".to_string()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| DockerBuildError::ArchiveIo)?;
        let StartExecResults::Attached { mut output, input } = self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|_| DockerBuildError::ArchiveIo)?
        else {
            return Err(DockerBuildError::ArchiveIo);
        };
        drop(input);

        let read = async {
            let mut bytes = Vec::new();
            while let Some(chunk) = output.next().await {
                let chunk = chunk.map_err(|_| DockerBuildError::ArchiveIo)?;
                let data = match chunk {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => message,
                    LogOutput::StdErr { .. } | LogOutput::StdIn { .. } => {
                        return Err(DockerBuildError::ArchiveIo);
                    }
                };
                if bytes.len().saturating_add(data.len()) > limit {
                    return Err(DockerBuildError::BuildOutputLimitExceeded);
                }
                bytes.extend_from_slice(&data);
                if let Some(complete) = complete_tar_length(&bytes) {
                    bytes.truncate(complete);
                    return Ok(bytes);
                }
            }
            Err(DockerBuildError::ArchiveIo)
        };
        tokio::time::timeout(std::time::Duration::from_secs(60), read)
            .await
            .map_err(|_| DockerBuildError::BuildTimeout)?
    }

    async fn collect_build_logs(
        &self,
        builder_id: &str,
    ) -> Result<Vec<PluginBuildLog>, BuildAdapterError> {
        let mut stream = self.docker.logs(
            builder_id,
            Some(
                LogsOptionsBuilder::new()
                    .follow(false)
                    .stdout(true)
                    .stderr(true)
                    .timestamps(false)
                    .build(),
            ),
        );
        let mut logs = Vec::new();
        let mut total = 0_usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| BuildAdapterError::new("BUILD_LOG_READ_FAILED"))?;
            let (stream_kind, bytes) = match chunk {
                LogOutput::StdErr { message } => (PluginBuildLogStream::Stderr, message),
                LogOutput::StdOut { message } | LogOutput::Console { message } => {
                    (PluginBuildLogStream::Stdout, message)
                }
                LogOutput::StdIn { .. } => continue,
            };
            total = total.saturating_add(bytes.len());
            if total > BUILD_LOG_LIMIT_BYTES {
                let _ = self
                    .docker
                    .stop_container(
                        builder_id,
                        Some(StopContainerOptionsBuilder::new().t(1).build()),
                    )
                    .await;
                logs.push(PluginBuildLog {
                    sequence: logs.len() as u64,
                    stream: PluginBuildLogStream::System,
                    message: "BUILD_LOG_LIMIT_EXCEEDED".to_string(),
                });
                return Err(BuildAdapterError::with_logs(
                    "BUILD_LOG_LIMIT_EXCEEDED",
                    logs,
                ));
            }
            for line in String::from_utf8_lossy(&bytes).lines() {
                logs.push(PluginBuildLog {
                    sequence: logs.len() as u64,
                    stream: stream_kind,
                    message: line.to_string(),
                });
            }
        }
        Ok(logs)
    }

    async fn download_output(&self, builder_id: &str) -> Result<Vec<u8>, BuildAdapterError> {
        self.download_exec_archive(
            builder_id,
            "output",
            BUILD_OUTPUT_LIMIT_BYTES as usize + 1024 * 1024,
        )
        .await
        .map_err(|error| BuildAdapterError::new(error.code()))
    }

    async fn verify_managed_image(
        &self,
        image_id: &str,
        expected: &HashMap<String, String>,
    ) -> Result<(), BuildAdapterError> {
        let labels = self
            .docker
            .inspect_image(image_id)
            .await
            .map_err(|_| BuildAdapterError::new("IMAGE_ATTESTATION_FAILED"))?
            .config
            .and_then(|config| config.labels)
            .ok_or_else(|| BuildAdapterError::new("IMAGE_ATTESTATION_FAILED"))?;
        if expected
            .iter()
            .all(|(key, value)| labels.get(key) == Some(value))
        {
            Ok(())
        } else {
            Err(BuildAdapterError::new("IMAGE_ATTESTATION_FAILED"))
        }
    }
}

#[async_trait]
impl InstallBuildAdapter for DockerBuildAdapter {
    async fn build(&self, request: BuildRequest) -> Result<BuildOutput, BuildAdapterError> {
        if request.installation_id != self.installation_id
            || request.prepared_request
                != self
                    .plugin_data
                    .join("prepared")
                    .join(format!("{}.json", request.operation_id))
        {
            return Err(BuildAdapterError::new("INVALID_BUILD_REQUEST"));
        }
        let result = self.execute_build(&request).await;
        let cleanup = self.cleanup_temporary_resources(request.operation_id).await;
        match (result, cleanup) {
            (Ok(output), Ok(())) => Ok(output),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    async fn remove_image(&self, image_id: &str) -> Result<(), BuildAdapterError> {
        use bollard::query_parameters::RemoveImageOptionsBuilder;

        self.docker
            .remove_image(
                image_id,
                Some(
                    RemoveImageOptionsBuilder::new()
                        .force(true)
                        .noprune(false)
                        .build(),
                ),
                None,
            )
            .await
            .map_err(|_| BuildAdapterError::new("DOCKER_IMAGE_REMOVE_FAILED"))?;
        Ok(())
    }

    async fn cleanup_temporary_resources(
        &self,
        operation_id: Uuid,
    ) -> Result<(), BuildAdapterError> {
        let names = operation_resource_names(operation_id);
        for container in [
            names.builder_container,
            names.assembler_container,
            names.proxy_container,
        ] {
            let _ = self
                .docker
                .remove_container(
                    &container,
                    Some(
                        bollard::query_parameters::RemoveContainerOptionsBuilder::new()
                            .force(true)
                            .v(true)
                            .build(),
                    ),
                )
                .await;
        }
        let _ = self.docker.remove_network(&names.internal_network).await;
        let _ = self.docker.remove_network(&names.egress_network).await;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DockerBuildError {
    #[error("build input is unsafe")]
    UnsafeBuildInput,
    #[error("build output is unsafe")]
    UnsafeBuildOutput,
    #[error("build output exceeds limits")]
    BuildOutputLimitExceeded,
    #[error("trusted image inputs are invalid")]
    TrustedImage,
    #[error("managed image metadata is invalid")]
    InvalidImageMetadata,
    #[error("archive operation failed")]
    ArchiveIo,
    #[error("build timed out")]
    BuildTimeout,
}

impl DockerBuildError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsafeBuildInput => "UNSAFE_BUILD_INPUT",
            Self::UnsafeBuildOutput => "UNSAFE_BUILD_OUTPUT",
            Self::BuildOutputLimitExceeded => "BUILD_OUTPUT_LIMIT_EXCEEDED",
            Self::TrustedImage => "TRUSTED_IMAGE_INVALID",
            Self::InvalidImageMetadata => "INVALID_IMAGE_METADATA",
            Self::ArchiveIo => "ARCHIVE_IO_FAILED",
            Self::BuildTimeout => "BUILD_TIMEOUT",
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildStatus {
    state: String,
    code: Option<String>,
}
