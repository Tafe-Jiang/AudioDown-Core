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
}
