//! Shared types used across all HTTP features

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;

/// HTTP methods supported
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl Default for HttpMethod {
    fn default() -> Self {
        Self::Get
    }
}

/// HTTP response from a completed request
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code
    pub status_code: u16,
    /// Response headers
    pub headers: HashMap<String, String>,
    /// Response body as raw bytes
    pub body: Vec<u8>,
    /// Content-Type header if present
    pub content_type: Option<String>,
}

impl HttpResponse {
    /// Get body as UTF-8 string (lossy conversion)
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// Check if response indicates success (2xx status)
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }
}

/// Errors that can occur during HTTP operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum HttpError {
    #[error("Plugin network policy is not registered: {plugin_id}")]
    PluginNetworkNotConfigured { plugin_id: String },

    #[error("Network capability is disabled for plugin: {plugin_id}")]
    PluginNetworkDisabled { plugin_id: String },

    #[error("Domain not whitelisted: {domain}")]
    DomainNotWhitelisted { domain: String },

    #[error("Domain requires user approval: {domain}")]
    DomainNeedsApproval { domain: String },

    #[error("Rate limit exceeded for domain: {domain}")]
    RateLimited { domain: String },

    #[error("Invalid URL: {reason}")]
    InvalidUrl { reason: String },

    #[error("DNS resolution failed for {host}: {reason}")]
    DnsResolutionFailed { host: String, reason: String },

    #[error("DNS resolution returned an unsafe address: {address}")]
    UnsafeResolvedAddress { address: IpAddr },

    #[error("Plugin redirect limit exceeded")]
    RedirectLimitExceeded,

    #[error("Pinned resolution is unavailable: {reason}")]
    PinnedResolutionUnavailable { reason: String },

    #[error("Security warning: {message}")]
    SecurityWarning { message: String },

    #[error("Request failed: {message}")]
    RequestFailed { message: String },

    /// The response body exceeded the caller's buffered-read ceiling,
    /// either by declaring too large a `Content-Length` or by crossing the
    /// limit mid-stream.
    ///
    /// Distinct from [`Self::RequestFailed`] because it is not a transport
    /// problem: retrying fetches the same oversized resource again, so a
    /// caller that treats it as retryable loops forever. Callers map it to
    /// a permanent refusal.
    #[error("Response body exceeds the {limit}-byte buffered response limit")]
    ResponseTooLarge { limit: usize },

    #[error("Request timed out")]
    Timeout,

    #[error("Request was cancelled")]
    Cancelled,
}
