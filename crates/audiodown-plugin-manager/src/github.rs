use std::sync::OnceLock;

use regex::Regex;
use url::Url;

use crate::PluginManagerError;

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
