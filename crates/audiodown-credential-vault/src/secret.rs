use std::{collections::HashSet, fmt};

use audiodown_domain::credential::CredentialKind;
use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString, SecretVec};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

pub const SECRET_PAYLOAD_VERSION: u16 = 1;

const MAX_COOKIES: usize = 128;
const MAX_COOKIE_NAME_BYTES: usize = 256;
const MAX_COOKIE_VALUE_BYTES: usize = 8 * 1024;
const MAX_COOKIE_HOST_BYTES: usize = 253;
const MAX_COOKIE_PATH_BYTES: usize = 2 * 1024;
const MAX_TOKEN_BYTES: usize = 16 * 1024;

pub struct CookieSecretRecord {
    name: String,
    value: SecretString,
    host: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<DateTime<Utc>>,
}

impl CookieSecretRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        value: SecretString,
        host: impl Into<String>,
        path: impl Into<String>,
        secure: bool,
        http_only: bool,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<Self, SecretPayloadError> {
        let name = name.into();
        let host = host.into().to_ascii_lowercase();
        let path = path.into();
        if !valid_cookie_name(&name)
            || value.expose_secret().is_empty()
            || value.expose_secret().len() > MAX_COOKIE_VALUE_BYTES
            || !valid_cookie_host(&host)
            || path.is_empty()
            || path.len() > MAX_COOKIE_PATH_BYTES
            || !path.starts_with('/')
        {
            return Err(SecretPayloadError::InvalidCookie);
        }
        Ok(Self {
            name,
            value,
            host,
            path,
            secure,
            http_only,
            expires_at,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn secure(&self) -> bool {
        self.secure
    }

    pub fn http_only(&self) -> bool {
        self.http_only
    }

    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }

    pub fn with_value<T>(&self, use_value: impl FnOnce(&str) -> T) -> T {
        use_value(self.value.expose_secret())
    }
}

impl fmt::Debug for CookieSecretRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CookieSecretRecord")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .field("host", &self.host)
            .field("path", &self.path)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

pub struct CookieCredentialSecret {
    cookies: Vec<CookieSecretRecord>,
}

impl CookieCredentialSecret {
    pub fn new(cookies: Vec<CookieSecretRecord>) -> Result<Self, SecretPayloadError> {
        if cookies.is_empty() || cookies.len() > MAX_COOKIES {
            return Err(SecretPayloadError::InvalidCookie);
        }
        let mut identities = HashSet::new();
        if cookies.iter().any(|cookie| {
            !identities.insert((
                cookie.name.as_str(),
                cookie.host.as_str(),
                cookie.path.as_str(),
            ))
        }) {
            return Err(SecretPayloadError::InvalidCookie);
        }
        Ok(Self { cookies })
    }

    pub fn cookies(&self) -> &[CookieSecretRecord] {
        &self.cookies
    }
}

impl fmt::Debug for CookieCredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CookieCredentialSecret")
            .field("version", &SECRET_PAYLOAD_VERSION)
            .field("cookies", &self.cookies)
            .finish()
    }
}

pub struct TokenCredentialSecret {
    value: SecretString,
}

impl TokenCredentialSecret {
    pub fn bearer(value: SecretString) -> Result<Self, SecretPayloadError> {
        if value.expose_secret().is_empty() || value.expose_secret().len() > MAX_TOKEN_BYTES {
            return Err(SecretPayloadError::InvalidToken);
        }
        Ok(Self { value })
    }

    pub fn with_value<T>(&self, use_value: impl FnOnce(&str) -> T) -> T {
        use_value(self.value.expose_secret())
    }
}

impl fmt::Debug for TokenCredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenCredentialSecret")
            .field("version", &SECRET_PAYLOAD_VERSION)
            .field("scheme", &"bearer")
            .field("value", &"[REDACTED]")
            .finish()
    }
}

pub(crate) enum CredentialSecret {
    Cookie(CookieCredentialSecret),
    Token(TokenCredentialSecret),
}

pub struct CredentialSecretGuard {
    secret: CredentialSecret,
}

impl CredentialSecretGuard {
    pub(crate) fn new(secret: CredentialSecret) -> Self {
        Self { secret }
    }

    pub fn cookie(&self) -> Option<&CookieCredentialSecret> {
        match &self.secret {
            CredentialSecret::Cookie(secret) => Some(secret),
            CredentialSecret::Token(_) => None,
        }
    }

    pub fn token(&self) -> Option<&TokenCredentialSecret> {
        match &self.secret {
            CredentialSecret::Cookie(_) => None,
            CredentialSecret::Token(secret) => Some(secret),
        }
    }
}

impl fmt::Debug for CredentialSecretGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.secret {
            CredentialSecret::Cookie(_) => CredentialKind::Cookie,
            CredentialSecret::Token(_) => CredentialKind::Token,
        };
        formatter
            .debug_struct("CredentialSecretGuard")
            .field("kind", &kind)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SecretPayloadError {
    #[error("credential Cookie payload is invalid")]
    InvalidCookie,
    #[error("credential Token payload is invalid")]
    InvalidToken,
    #[error("credential secret payload is malformed")]
    Malformed,
    #[error("credential secret payload version is unsupported")]
    UnsupportedVersion,
    #[error("credential secret kind does not match its record")]
    KindMismatch,
}

pub(crate) fn encode_cookie(
    secret: &CookieCredentialSecret,
) -> Result<SecretVec<u8>, SecretPayloadError> {
    let cookies = secret
        .cookies
        .iter()
        .map(|cookie| CookiePayloadRef {
            name: &cookie.name,
            value: cookie.value.expose_secret(),
            host: &cookie.host,
            path: &cookie.path,
            secure: cookie.secure,
            http_only: cookie.http_only,
            expires_at: cookie.expires_at,
        })
        .collect();
    encode(&SecretPayloadRef::Cookie {
        version: SECRET_PAYLOAD_VERSION,
        cookies,
    })
}

pub(crate) fn encode_token(
    secret: &TokenCredentialSecret,
) -> Result<SecretVec<u8>, SecretPayloadError> {
    encode(&SecretPayloadRef::Token {
        version: SECRET_PAYLOAD_VERSION,
        scheme: "bearer",
        value: secret.value.expose_secret(),
    })
}

pub(crate) fn decode(
    kind: CredentialKind,
    plaintext: &SecretVec<u8>,
) -> Result<CredentialSecretGuard, SecretPayloadError> {
    let payload: SecretPayloadOwned = serde_json::from_slice(plaintext.expose_secret())
        .map_err(|_| SecretPayloadError::Malformed)?;
    match (kind, payload) {
        (CredentialKind::Cookie, SecretPayloadOwned::Cookie { version, cookies }) => {
            validate_version(version)?;
            let cookies = cookies
                .into_iter()
                .map(|cookie| {
                    CookieSecretRecord::new(
                        cookie.name,
                        SecretString::new(cookie.value.to_string()),
                        cookie.host,
                        cookie.path,
                        cookie.secure,
                        cookie.http_only,
                        cookie.expires_at,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CredentialSecretGuard::new(CredentialSecret::Cookie(
                CookieCredentialSecret::new(cookies)?,
            )))
        }
        (
            CredentialKind::Token,
            SecretPayloadOwned::Token {
                version,
                scheme,
                value,
            },
        ) => {
            validate_version(version)?;
            if scheme != "bearer" {
                return Err(SecretPayloadError::InvalidToken);
            }
            Ok(CredentialSecretGuard::new(CredentialSecret::Token(
                TokenCredentialSecret::bearer(SecretString::new(value.to_string()))?,
            )))
        }
        _ => Err(SecretPayloadError::KindMismatch),
    }
}

fn encode(payload: &SecretPayloadRef<'_>) -> Result<SecretVec<u8>, SecretPayloadError> {
    let mut encoded = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *encoded, payload).map_err(|_| SecretPayloadError::Malformed)?;
    Ok(SecretVec::new(encoded.to_vec()))
}

fn validate_version(version: u16) -> Result<(), SecretPayloadError> {
    if version == SECRET_PAYLOAD_VERSION {
        Ok(())
    } else {
        Err(SecretPayloadError::UnsupportedVersion)
    }
}

fn valid_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_COOKIE_NAME_BYTES
        && name.bytes().all(|byte| {
            byte > 0x20
                && byte < 0x7f
                && !matches!(
                    byte,
                    b'(' | b')'
                        | b'<'
                        | b'>'
                        | b'@'
                        | b','
                        | b';'
                        | b':'
                        | b'\\'
                        | b'"'
                        | b'/'
                        | b'['
                        | b']'
                        | b'?'
                        | b'='
                        | b'{'
                        | b'}'
                )
        })
}

fn valid_cookie_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_COOKIE_HOST_BYTES
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && host.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum SecretPayloadRef<'a> {
    Cookie {
        version: u16,
        cookies: Vec<CookiePayloadRef<'a>>,
    },
    Token {
        version: u16,
        scheme: &'static str,
        value: &'a str,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CookiePayloadRef<'a> {
    name: &'a str,
    value: &'a str,
    host: &'a str,
    path: &'a str,
    secure: bool,
    http_only: bool,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum SecretPayloadOwned {
    Cookie {
        version: u16,
        cookies: Vec<CookiePayloadOwned>,
    },
    Token {
        version: u16,
        scheme: String,
        value: Zeroizing<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CookiePayloadOwned {
    name: String,
    value: Zeroizing<String>,
    host: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<DateTime<Utc>>,
}
