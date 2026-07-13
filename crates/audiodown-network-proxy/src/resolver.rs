use std::{collections::HashMap, net::IpAddr};

use thiserror::Error;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    #[error("host could not be resolved")]
    NotFound,
}

pub trait DnsResolver {
    fn resolve(&mut self, host: &str) -> Result<Vec<IpAddr>, ResolveError>;
}

#[derive(Debug, Clone, Default)]
pub struct StaticResolver {
    answers: HashMap<String, Vec<IpAddr>>,
}

impl StaticResolver {
    pub fn new<I, H>(answers: I) -> Self
    where
        I: IntoIterator<Item = (H, Vec<IpAddr>)>,
        H: Into<String>,
    {
        Self {
            answers: answers
                .into_iter()
                .map(|(host, addresses)| (normalize_host(host.into().as_str()), addresses))
                .collect(),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn set(&mut self, host: impl AsRef<str>, addresses: Vec<IpAddr>) {
        self.answers
            .insert(normalize_host(host.as_ref()), addresses);
    }
}

impl DnsResolver for StaticResolver {
    fn resolve(&mut self, host: &str) -> Result<Vec<IpAddr>, ResolveError> {
        self.answers
            .get(&normalize_host(host))
            .cloned()
            .ok_or(ResolveError::NotFound)
    }
}

fn normalize_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}
