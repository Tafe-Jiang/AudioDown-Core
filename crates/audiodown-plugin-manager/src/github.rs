use std::{path::Path, sync::OnceLock, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use regex::Regex;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::{DownloadedSnapshot, PluginManagerError, RepositorySource};

const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    api_base: Url,
    archive_base: Url,
}

impl GitHubClient {
    pub fn new(api_base: &str, archive_base: &str) -> Result<Self, PluginManagerError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("AudioDown-Core/1.0")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| PluginManagerError::HttpClient)?;

        Ok(Self {
            client,
            api_base: parse_service_base(api_base)?,
            archive_base: parse_service_base(archive_base)?,
        })
    }

    async fn get_json<T>(&self, url: Url) -> Result<T, PluginManagerError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| PluginManagerError::RepositoryRequest)?;
        if !response.status().is_success() {
            return Err(PluginManagerError::NonSuccessResponse);
        }
        response
            .json()
            .await
            .map_err(|_| PluginManagerError::InvalidResponse)
    }

    async fn download_archive(
        &self,
        url: Url,
        destination: &Path,
    ) -> Result<std::path::PathBuf, PluginManagerError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| PluginManagerError::RepositoryRequest)?;
        if !response.status().is_success() {
            return Err(PluginManagerError::NonSuccessResponse);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
        {
            return Err(PluginManagerError::ArchiveTooLarge);
        }

        let temporary_path = destination.join("snapshot.tar.gz.tmp");
        let archive_path = destination.join("snapshot.tar.gz");
        let result = async {
            let mut file = tokio::fs::File::create(&temporary_path)
                .await
                .map_err(|_| PluginManagerError::SnapshotIo)?;
            let mut size = 0_u64;
            let mut chunks = response.bytes_stream();

            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|_| PluginManagerError::RepositoryRequest)?;
                size = size
                    .checked_add(chunk.len() as u64)
                    .ok_or(PluginManagerError::ArchiveTooLarge)?;
                if size > MAX_ARCHIVE_BYTES {
                    return Err(PluginManagerError::ArchiveTooLarge);
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|_| PluginManagerError::SnapshotIo)?;
            }

            file.sync_all()
                .await
                .map_err(|_| PluginManagerError::SnapshotIo)?;
            drop(file);
            tokio::fs::rename(&temporary_path, &archive_path)
                .await
                .map_err(|_| PluginManagerError::SnapshotIo)?;
            Ok(archive_path.clone())
        }
        .await;

        if result.is_err() {
            let _ = tokio::fs::remove_file(&temporary_path).await;
        }
        result
    }
}

#[async_trait]
impl RepositorySource for GitHubClient {
    async fn resolve_and_download(
        &self,
        source: &GitHubRepositoryRef,
        destination: &Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError> {
        let repository_url = endpoint(
            &self.api_base,
            &["repos", source.owner(), source.repository()],
        )?;
        let repository: RepositoryResponse = self.get_json(repository_url).await?;
        let default_branch = repository
            .default_branch
            .filter(|branch| !branch.is_empty())
            .ok_or(PluginManagerError::MissingDefaultBranch)?;

        let commit_url = endpoint(
            &self.api_base,
            &[
                "repos",
                source.owner(),
                source.repository(),
                "commits",
                &default_branch,
            ],
        )?;
        let commit: CommitResponse = self.get_json(commit_url).await?;
        if !valid_commit_sha(&commit.sha) {
            return Err(PluginManagerError::InvalidCommitSha);
        }

        let archive_url = endpoint(
            &self.archive_base,
            &[source.owner(), source.repository(), "tar.gz", &commit.sha],
        )?;
        let archive_path = self.download_archive(archive_url, destination).await?;

        Ok(DownloadedSnapshot {
            commit_sha: commit.sha,
            archive_path,
        })
    }
}

#[derive(Deserialize)]
struct RepositoryResponse {
    default_branch: Option<String>,
}

#[derive(Deserialize)]
struct CommitResponse {
    sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepositoryRef {
    owner: String,
    repository: String,
    canonical_url: String,
}

impl GitHubRepositoryRef {
    pub fn parse(input: &str) -> Result<Self, PluginManagerError> {
        let authority = input
            .strip_prefix("https://")
            .and_then(|rest| rest.split('/').next())
            .ok_or(PluginManagerError::InvalidRepositoryUrl)?;
        if authority != "github.com" {
            return Err(PluginManagerError::InvalidRepositoryUrl);
        }

        let url = Url::parse(input).map_err(|_| PluginManagerError::InvalidRepositoryUrl)?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(PluginManagerError::InvalidRepositoryUrl);
        }

        let mut segments = url
            .path_segments()
            .ok_or(PluginManagerError::InvalidRepositoryUrl)?
            .collect::<Vec<_>>();
        if segments.last() == Some(&"") {
            segments.pop();
        }
        if segments.len() != 2 {
            return Err(PluginManagerError::InvalidRepositoryUrl);
        }

        let owner = segments[0];
        let repository = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
        if !valid_segment(owner) || !valid_segment(repository) {
            return Err(PluginManagerError::InvalidRepositoryUrl);
        }

        Ok(Self {
            owner: owner.to_string(),
            repository: repository.to_string(),
            canonical_url: format!("https://github.com/{owner}/{repository}"),
        })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn canonical_url(&self) -> &str {
        &self.canonical_url
    }
}

fn valid_segment(value: &str) -> bool {
    static SEGMENT_PATTERN: OnceLock<Regex> = OnceLock::new();
    SEGMENT_PATTERN
        .get_or_init(|| Regex::new(r"^[A-Za-z0-9_.-]{1,100}$").expect("valid segment regex"))
        .is_match(value)
}

fn valid_commit_sha(value: &str) -> bool {
    static SHA_PATTERN: OnceLock<Regex> = OnceLock::new();
    SHA_PATTERN
        .get_or_init(|| Regex::new(r"^[0-9a-f]{40}$").expect("valid commit SHA regex"))
        .is_match(value)
}

fn parse_service_base(value: &str) -> Result<Url, PluginManagerError> {
    let mut url = Url::parse(value).map_err(|_| PluginManagerError::InvalidServiceBaseUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PluginManagerError::InvalidServiceBaseUrl);
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn endpoint(base: &Url, segments: &[&str]) -> Result<Url, PluginManagerError> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|_| PluginManagerError::InvalidServiceBaseUrl)?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}
