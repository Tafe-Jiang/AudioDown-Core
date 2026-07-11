#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use thiserror::Error;

pub mod github;

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
}
