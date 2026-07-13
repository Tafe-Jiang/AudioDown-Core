use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use audiodown_plugin_api::manifest::PluginManifest;
use thiserror::Error;
use url::{Host, Url};

use crate::resolver::DnsResolver;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProxyPolicyError {
    #[error("proxy URL is invalid")]
    InvalidUrl,
    #[error("proxy host is not allowed by the plugin manifest")]
    HostNotAllowed,
    #[error("proxy target address is blocked")]
    BlockedAddress,
    #[error("proxy DNS resolution failed")]
    ResolveFailed,
    #[error("proxy DNS answer changed after address pinning")]
    DnsRebinding,
    #[error("proxy redirect changed host")]
    RedirectHostChanged,
    #[error("developer mode is required for fixture mappings")]
    DeveloperModeRequired,
    #[error("fixture mapping must target one exact hostname")]
    InvalidFixtureMapping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowedHosts {
    patterns: Vec<HostPattern>,
}

impl AllowedHosts {
    fn from_manifest(manifest: &PluginManifest) -> Self {
        Self {
            patterns: manifest
                .network
                .allowed_hosts
                .iter()
                .map(|pattern| HostPattern::parse(pattern))
                .collect(),
        }
    }

    fn allows(&self, host: &str) -> bool {
        self.patterns.iter().any(|pattern| pattern.matches(host))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostPattern {
    Exact(String),
    Wildcard {
        suffix: String,
        suffix_labels: usize,
    },
    Invalid,
}

impl HostPattern {
    fn parse(pattern: &str) -> Self {
        let pattern = normalize_host(pattern);
        if pattern.is_empty() || pattern.contains('/') || pattern.contains(':') {
            return Self::Invalid;
        }
        if let Some(suffix) = pattern.strip_prefix("*.") {
            if !valid_hostname(suffix) {
                return Self::Invalid;
            }
            Self::Wildcard {
                suffix: suffix.to_string(),
                suffix_labels: suffix.split('.').count(),
            }
        } else if valid_hostname(&pattern) {
            Self::Exact(pattern)
        } else {
            Self::Invalid
        }
    }

    fn matches(&self, host: &str) -> bool {
        let host = normalize_host(host);
        match self {
            Self::Exact(pattern) => host == *pattern,
            Self::Wildcard {
                suffix,
                suffix_labels,
            } => {
                host.ends_with(&format!(".{suffix}"))
                    && host.split('.').count() == suffix_labels + 1
            }
            Self::Invalid => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProxyPolicy {
    allowed_hosts: AllowedHosts,
    developer_mode: bool,
    fixture_mappings: HashMap<String, IpAddr>,
}

impl ProxyPolicy {
    pub fn production(manifest: &PluginManifest) -> Self {
        Self {
            allowed_hosts: AllowedHosts::from_manifest(manifest),
            developer_mode: false,
            fixture_mappings: HashMap::new(),
        }
    }

    pub fn developer(manifest: &PluginManifest) -> Self {
        Self {
            allowed_hosts: AllowedHosts::from_manifest(manifest),
            developer_mode: true,
            fixture_mappings: HashMap::new(),
        }
    }

    pub fn with_fixture_mapping(
        mut self,
        host: impl AsRef<str>,
        address: IpAddr,
    ) -> Result<Self, ProxyPolicyError> {
        if !self.developer_mode {
            return Err(ProxyPolicyError::DeveloperModeRequired);
        }
        let host = normalize_host(host.as_ref());
        if !valid_hostname(&host) || !is_fixture_address(address) {
            return Err(ProxyPolicyError::InvalidFixtureMapping);
        }
        self.fixture_mappings.insert(host, address);
        Ok(self)
    }

    pub fn authorize_url(
        &self,
        raw_url: &str,
        resolver: &mut impl DnsResolver,
    ) -> Result<PinnedTarget, ProxyPolicyError> {
        let parsed = parse_request_url(raw_url, self.developer_mode)?;
        if parsed.url.scheme() == "http" && !self.fixture_mappings.contains_key(&parsed.host) {
            return Err(ProxyPolicyError::InvalidUrl);
        }
        if !self.allowed_hosts.allows(&parsed.host) {
            return Err(ProxyPolicyError::HostNotAllowed);
        }
        let addresses = self.resolve_and_validate(&parsed.host, resolver)?;
        Ok(PinnedTarget {
            url: parsed.url,
            host: parsed.host,
            port: parsed.port,
            addresses,
        })
    }

    pub fn authorize_redirect(
        &self,
        pinned: &PinnedTarget,
        raw_url: &str,
        resolver: &mut impl DnsResolver,
    ) -> Result<PinnedTarget, ProxyPolicyError> {
        let redirected = self.authorize_url(raw_url, resolver)?;
        if redirected.host != pinned.host {
            return Err(ProxyPolicyError::RedirectHostChanged);
        }
        if address_set(&redirected.addresses) != address_set(&pinned.addresses) {
            return Err(ProxyPolicyError::DnsRebinding);
        }
        Ok(redirected)
    }

    fn resolve_and_validate(
        &self,
        host: &str,
        resolver: &mut impl DnsResolver,
    ) -> Result<Vec<IpAddr>, ProxyPolicyError> {
        let fixture_address = self.fixture_mappings.get(host).copied();
        let mut addresses = if let Some(address) = fixture_address {
            vec![address]
        } else {
            resolver
                .resolve(host)
                .map_err(|_| ProxyPolicyError::ResolveFailed)?
        };
        let address_is_allowed = if fixture_address.is_some() {
            is_fixture_address
        } else {
            is_global_address
        };
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|address| !address_is_allowed(*address))
        {
            return Err(ProxyPolicyError::BlockedAddress);
        }
        addresses.sort_unstable();
        addresses.dedup();
        Ok(addresses)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedTarget {
    url: Url,
    host: String,
    port: u16,
    addresses: Vec<IpAddr>,
}

impl PinnedTarget {
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn pinned_addresses(&self) -> &[IpAddr] {
        &self.addresses
    }
}

struct ParsedRequestUrl {
    url: Url,
    host: String,
    port: u16,
}

fn parse_request_url(
    raw_url: &str,
    developer_mode: bool,
) -> Result<ParsedRequestUrl, ProxyPolicyError> {
    let url = Url::parse(raw_url).map_err(|_| ProxyPolicyError::InvalidUrl)?;
    if (!developer_mode && url.scheme() != "https")
        || (developer_mode && !matches!(url.scheme(), "http" | "https"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ProxyPolicyError::InvalidUrl);
    }
    let host = match url.host().ok_or(ProxyPolicyError::InvalidUrl)? {
        Host::Domain(host) => normalize_host(host),
        Host::Ipv4(_) | Host::Ipv6(_) => return Err(ProxyPolicyError::InvalidUrl),
    };
    if !valid_hostname(&host) {
        return Err(ProxyPolicyError::InvalidUrl);
    }
    let port = url
        .port_or_known_default()
        .ok_or(ProxyPolicyError::InvalidUrl)?;
    if port == 0 {
        return Err(ProxyPolicyError::InvalidUrl);
    }
    Ok(ParsedRequestUrl { url, host, port })
}

fn address_set(addresses: &[IpAddr]) -> BTreeSet<IpAddr> {
    addresses.iter().copied().collect()
}

fn normalize_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

fn valid_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.parse::<IpAddr>().is_ok() {
        return false;
    }
    host.split('.').all(valid_label)
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_global_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_global_ipv4(address),
        IpAddr::V6(address) => is_global_ipv6(address),
    }
}

fn is_fixture_address(address: IpAddr) -> bool {
    is_global_address(address) || address.is_loopback()
}

fn is_global_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_broadcast()
        || octets[0] == 0
        || octets[0] >= 240
        || octets[0] == 127
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 169 && octets[1] == 254)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 18)
        || (octets[0] == 198 && octets[1] == 19)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113))
}

fn is_global_ipv6(address: Ipv6Addr) -> bool {
    if let Some(address) = address.to_ipv4() {
        return is_global_ipv4(address);
    }
    let segments = address.segments();
    let is_special_2001 =
        segments[0] == 0x2001 && ((segments[1] & 0xfe00) == 0 || segments[1] == 0x0db8);
    let is_six_to_four = segments[0] == 0x2002;
    let is_documentation = segments[0] == 0x3fff && (segments[1] & 0xf000) == 0;

    (segments[0] & 0xe000) == 0x2000 && !(is_special_2001 || is_six_to_four || is_documentation)
}
