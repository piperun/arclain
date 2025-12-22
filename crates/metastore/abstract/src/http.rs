//! HTTP abstraction layer
//!
//! Providers return HttpRequest instructions, callers execute them.

use std::collections::HashMap;

/// HTTP request instruction (provider returns this, caller executes)
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// URL to fetch
    pub url: String,
    /// HTTP method
    pub method: HttpMethod,
    /// Headers to send
    pub headers: HashMap<String, String>,
    /// Cache key for storing response
    pub cache_key: String,
}

impl HttpRequest {
    pub fn get(url: &str, cache_key: &str) -> Self {
        Self {
            url: url.to_string(),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            cache_key: cache_key.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

/// HTTP response from executing a request
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

impl HttpResponse {
    pub fn body_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.body)
    }
}

/// Trait for HTTP backends to implement
pub trait HttpClient: Send + Sync {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

#[derive(Debug)]
pub enum HttpError {
    Network(String),
    Timeout,
    InvalidUrl,
    Other(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {}", msg),
            Self::Timeout => write!(f, "Request timed out"),
            Self::InvalidUrl => write!(f, "Invalid URL"),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}
impl std::error::Error for HttpError {}
