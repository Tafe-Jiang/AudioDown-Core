use thiserror::Error;

use crate::policy::ProxyPolicyError;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum HttpProxyError {
    #[error(transparent)]
    Policy(#[from] ProxyPolicyError),
    #[error("proxy HTTP method is not allowed")]
    MethodNotAllowed,
    #[error("proxy request headers are invalid or too large")]
    RequestHeadersTooLarge,
    #[error("proxy request body is too large")]
    RequestBodyTooLarge,
    #[error("proxy redirect limit exceeded")]
    TooManyRedirects,
    #[error("proxy redirect is invalid")]
    InvalidRedirect,
    #[error("proxy response headers are invalid or too large")]
    ResponseHeadersTooLarge,
    #[error("proxy response body is too large")]
    ResponseBodyTooLarge,
    #[error("proxy request timed out")]
    Timeout,
    #[error("proxy concurrency limit reached")]
    ConcurrencyLimited,
    #[error("proxy response encoding is invalid")]
    InvalidResponseEncoding,
    #[error("proxy transport failed")]
    Transport,
}
