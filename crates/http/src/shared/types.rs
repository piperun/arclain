//! Shared types used across all HTTP features

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    #[error("Domain not whitelisted: {domain}")]
    DomainNotWhitelisted { domain: String },

    #[error("Domain requires user approval: {domain}")]
    DomainNeedsApproval { domain: String },

    #[error("Rate limit exceeded for domain: {domain}")]
    RateLimited { domain: String },

    #[error("Invalid URL: {reason}")]
    InvalidUrl { reason: String },

    #[error("Security warning: {message}")]
    SecurityWarning { message: String },

    #[error("Request failed: {message}")]
    RequestFailed { message: String },

    #[error("Request timed out")]
    Timeout,

    #[error("Request was cancelled")]
    Cancelled,
}
