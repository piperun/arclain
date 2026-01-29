//! Error types for gameta

use std::fmt;

/// Parse error types
#[derive(Debug, Clone)]
pub enum ParseError {
    /// Invalid response format
    InvalidFormat(String),
    /// Content is geo-blocked
    Geoblocked(String),
    /// Missing required data
    MissingData(String),
    /// Network error during parsing (e.g., failed to fetch required data)
    NetworkError(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(msg) => write!(f, "Invalid format: {}", msg),
            Self::Geoblocked(msg) => write!(f, "Geo-blocked: {}", msg),
            Self::MissingData(msg) => write!(f, "Missing data: {}", msg),
            Self::NetworkError(msg) => write!(f, "Network error: {}", msg),
        }
    }
}

impl std::error::Error for ParseError {}

/// Storage error types
#[derive(Debug)]
pub enum StorageError {
    NotFound,
    ConnectionFailed(String),
    QueryFailed(String),
    SerializationError(String),
    Other(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "Not found"),
            Self::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            Self::QueryFailed(msg) => write!(f, "Query failed: {}", msg),
            Self::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for StorageError {}

/// HTTP error types
#[derive(Debug)]
pub enum HttpError {
    Network(String),
    Timeout,
    InvalidUrl,
    StatusCode(u16, String),
    Other(String),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {}", msg),
            Self::Timeout => write!(f, "Request timed out"),
            Self::InvalidUrl => write!(f, "Invalid URL"),
            Self::StatusCode(code, msg) => write!(f, "HTTP {}: {}", code, msg),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for HttpError {}
