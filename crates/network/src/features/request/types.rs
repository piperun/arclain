//! Types for HTTP requests

use crate::shared::HttpMethod;
use std::collections::HashMap;
use std::time::Duration;

/// Unique identifier for a pending request
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

impl RequestId {
    /// Generate a new unique request ID
    pub fn new() -> Self {
        Self(uuid_simple())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple UUID-ish generator (no external dependency)
fn uuid_simple() -> String {
    use std::time::SystemTime;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // Add some randomness from thread ID and a counter
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    format!("{:x}-{:x}", timestamp, count)
}

/// Configuration for an HTTP request
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// The URL to request
    pub url: String,
    /// HTTP method
    pub method: HttpMethod,
    /// Request headers
    pub headers: HashMap<String, String>,
    /// Request body (for POST, PUT, etc.)
    pub body: Option<Vec<u8>>,
    /// Request timeout
    pub timeout: Duration,
}

impl HttpRequest {
    /// Create a simple GET request
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            body: None,
            timeout: Duration::from_secs(30),
        }
    }

    /// Create a POST request with body
    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            url: url.into(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            body: Some(body),
            timeout: Duration::from_secs(30),
        }
    }

    /// Add a header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Status of a pending request
#[derive(Debug, Clone)]
pub enum RequestStatus {
    /// Request is queued but not started
    Pending,
    /// Request is in progress
    InProgress,
    /// Request completed successfully
    Ready(crate::shared::HttpResponse),
    /// Request failed
    Failed(String),
    /// Request was cancelled
    Cancelled,
}

impl RequestStatus {
    /// Check if request is still pending or in progress
    pub fn is_pending(&self) -> bool {
        matches!(self, RequestStatus::Pending | RequestStatus::InProgress)
    }

    /// Check if request is complete (success or failure)
    pub fn is_complete(&self) -> bool {
        matches!(
            self,
            RequestStatus::Ready(_) | RequestStatus::Failed(_) | RequestStatus::Cancelled
        )
    }
}
