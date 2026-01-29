//! HTTP abstraction layer
//!
//! Providers return HttpRequest instructions, callers execute them.
//! This decouples provider logic from HTTP execution.

use std::collections::HashMap;

use crate::errors::HttpError;

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
    /// Create a GET request
    pub fn get(url: &str, cache_key: &str) -> Self {
        Self {
            url: url.to_string(),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            cache_key: cache_key.to_string(),
        }
    }

    /// Create a POST request
    pub fn post(url: &str, cache_key: &str) -> Self {
        Self {
            url: url.to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            cache_key: cache_key.to_string(),
        }
    }

    /// Add a header
    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }
}

/// HTTP method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

/// HTTP response from executing a request
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code
    pub status: u16,
    /// Response body
    pub body: Vec<u8>,
    /// Content-Type header
    pub content_type: Option<String>,
}

impl HttpResponse {
    /// Create a successful response
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            body,
            content_type: None,
        }
    }

    /// Create an error response
    pub fn error(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
            content_type: None,
        }
    }

    /// Get body as string
    pub fn body_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.body)
    }

    /// Check if response is successful (2xx)
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Trait for HTTP backends to implement
pub trait HttpClient: Send + Sync {
    /// Execute an HTTP request
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;
}
