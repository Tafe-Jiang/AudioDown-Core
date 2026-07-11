use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

pub const NODE22_BASE_TAG: &str = "node:22-bookworm-slim";
pub const BUILDER_IMAGE: &str = "audiodown/plugin-builder-node22:1.0";
pub const RUNTIME_IMAGE: &str = "audiodown/plugin-runtime-node22:1.0";
pub const POLICY_VERSION: &str = "1.0";

const TRUSTED_IMAGE_LABEL: &str = "io.audiodown.trusted-image";
const IMAGE_KIND_LABEL: &str = "io.audiodown.trusted-image-kind";
const BASE_DIGEST_LABEL: &str = "io.audiodown.base-image-digest";
const SDK_HASH_LABEL: &str = "io.audiodown.sdk-hash";
const POLICY_VERSION_LABEL: &str = "io.audiodown.build-policy-version";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeImageLock {
    pub image: String,
    pub digest: String,
}

impl NodeImageLock {
    pub fn parse(json: &str) -> Result<Self, TrustedImageError> {
        let lock: Self = serde_json::from_str(json)?;
        lock.validate()?;
        Ok(lock)
    }

    pub fn embedded() -> Result<Self, TrustedImageError> {
        Self::parse(include_str!(
            "../../../docker/plugin-runtime/node22.lock.json"
        ))
    }

    fn validate(&self) -> Result<(), TrustedImageError> {
        if self.image != NODE22_BASE_TAG {
            return Err(TrustedImageError::UnexpectedBaseImage);
        }
        if !is_sha256_digest(&self.digest) {
            return Err(TrustedImageError::InvalidDigest);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedImageKind {
    Builder,
    Runtime,
}

impl TrustedImageKind {
    fn label_value(self) -> &'static str {
        match self {
            Self::Builder => "node22-builder",
            Self::Runtime => "node22-runtime",
        }
    }
}

pub fn pinned_base_reference(lock: &NodeImageLock) -> Result<String, TrustedImageError> {
    lock.validate()?;
    Ok(format!("{}@{}", lock.image, lock.digest))
}

pub fn verify_repo_digests(
    lock: &NodeImageLock,
    repo_digests: &[String],
) -> Result<(), TrustedImageError> {
    lock.validate()?;
    let expected = pinned_base_reference(lock)?;
    let repository = lock
        .image
        .split_once(':')
        .map(|(repository, _)| repository)
        .unwrap_or(lock.image.as_str());
    let canonical = format!("{repository}@{}", lock.digest);
    if repo_digests
        .iter()
        .any(|digest| digest == &expected || digest == &canonical)
    {
        Ok(())
    } else {
        Err(TrustedImageError::RepoDigestMismatch)
    }
}

pub fn trusted_image_labels(
    kind: TrustedImageKind,
    base_digest: &str,
    sdk_hash: &str,
) -> HashMap<String, String> {
    HashMap::from([
        (TRUSTED_IMAGE_LABEL.to_string(), "true".to_string()),
        (IMAGE_KIND_LABEL.to_string(), kind.label_value().to_string()),
        (BASE_DIGEST_LABEL.to_string(), base_digest.to_string()),
        (SDK_HASH_LABEL.to_string(), sdk_hash.to_string()),
        (POLICY_VERSION_LABEL.to_string(), POLICY_VERSION.to_string()),
    ])
}

pub fn verify_trusted_image_labels(
    kind: TrustedImageKind,
    base_digest: &str,
    sdk_hash: &str,
    labels: &HashMap<String, String>,
) -> Result<(), TrustedImageError> {
    if !is_sha256_digest(base_digest) || !is_lower_hex_sha256(sdk_hash) {
        return Err(TrustedImageError::InvalidAttestation);
    }
    let expected = trusted_image_labels(kind, base_digest, sdk_hash);
    if expected
        .iter()
        .all(|(key, value)| labels.get(key) == Some(value))
    {
        Ok(())
    } else {
        Err(TrustedImageError::LabelMismatch)
    }
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(is_lower_hex_sha256)
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Error)]
pub enum TrustedImageError {
    #[error("trusted image lock must name node:22-bookworm-slim")]
    UnexpectedBaseImage,
    #[error("trusted image lock digest must be a lowercase SHA-256 digest")]
    InvalidDigest,
    #[error("pulled image RepoDigests do not contain the locked digest")]
    RepoDigestMismatch,
    #[error("trusted image attestation values are invalid")]
    InvalidAttestation,
    #[error("trusted image labels do not match the fixed build inputs")]
    LabelMismatch,
    #[error("trusted image lock is invalid JSON")]
    Json(#[from] serde_json::Error),
}
