#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use thiserror::Error;

pub mod archive;
pub mod github;
mod package;
pub mod service;
pub mod staging;
pub mod validation;

#[async_trait]
pub trait RepositorySource: Send + Sync {
    async fn resolve_and_download(
        &self,
        source: &github::GitHubRepositoryRef,
        destination: &Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedSnapshot {
    pub commit_sha: String,
    pub archive_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum PluginManagerError {
    #[error("invalid public GitHub repository URL")]
    InvalidRepositoryUrl,
    #[error("invalid repository service base URL")]
    InvalidServiceBaseUrl,
    #[error("failed to create the repository HTTP client")]
    HttpClient,
    #[error("repository request failed")]
    RepositoryRequest,
    #[error("repository response returned a non-success status")]
    NonSuccessResponse,
    #[error("repository response was invalid")]
    InvalidResponse,
    #[error("repository default branch is missing")]
    MissingDefaultBranch,
    #[error("repository commit SHA is invalid")]
    InvalidCommitSha,
    #[error("repository archive exceeds the compressed size limit")]
    ArchiveTooLarge,
    #[error("snapshot filesystem operation failed")]
    SnapshotIo,
    #[error("repository archive exceeds the compressed size limit")]
    ArchiveCompressedTooLarge,
    #[error("repository archive path escapes its root")]
    ArchivePathEscape,
    #[error("repository archive contains an unsupported entry")]
    ArchiveUnsupportedEntry,
    #[error("repository archive contains multiple top-level directories")]
    ArchiveMultipleRoots,
    #[error("repository archive contains a duplicate normalized path")]
    ArchiveDuplicatePath,
    #[error("repository archive contains a case-folded path conflict")]
    ArchiveCaseConflict,
    #[error("repository archive contains a non-UTF-8 path")]
    ArchiveNonUtf8Path,
    #[error("repository archive contains too many files")]
    ArchiveTooManyFiles,
    #[error("repository archive contains an oversized file")]
    ArchiveFileTooLarge,
    #[error("repository archive exceeds the extracted size limit")]
    ArchiveExtractedTooLarge,
    #[error("repository archive is invalid")]
    ArchiveInvalid,
    #[error("repository archive filesystem operation failed")]
    ArchiveIo,
    #[error("repository index is invalid")]
    InvalidRepositoryIndex,
    #[error("plugin path is invalid")]
    InvalidPluginPath,
    #[error("plugin manifest is invalid")]
    InvalidPluginManifest,
    #[error("plugin is incompatible with this Core or plugin API version")]
    IncompatiblePlugin,
    #[error("repository contains duplicate plugin identifiers")]
    DuplicatePlugin,
    #[error("plugin package metadata is invalid")]
    InvalidPackage,
    #[error("plugin tree contains a forbidden file or entry")]
    ForbiddenPluginFile,
    #[error("staging metadata is invalid")]
    InvalidStagingMetadata,
    #[error("staged snapshot was not found")]
    SnapshotNotFound,
    #[error("lifecycle-script risk grant does not match the staged plugin")]
    RiskGrantMismatch,
    #[error("plugin state store is unavailable")]
    PluginStateUnavailable,
    #[error("plugin runtime control is unavailable")]
    RuntimeUnavailable,
}
