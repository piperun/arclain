//! Provider trait definition

use metastore_abstract::{HttpRequest, HttpResponse};
use metastore_types::{MetadataSource, ProductMetadata, SearchResult};

/// Trait that all metadata providers must implement
pub trait MetadataProvider: Send + Sync {
    /// Provider identifier
    fn id(&self) -> MetadataSource;

    /// Detect product ID from text (e.g., filename)
    /// Returns the external ID if detected (e.g., "RJ123456")
    fn detect(&self, text: &str) -> Option<String>;

    /// Get HTTP requests needed to fetch metadata
    fn request_metadata(&self, external_id: &str) -> Vec<HttpRequest>;

    /// Parse responses into ProductMetadata
    fn parse_responses(
        &self,
        external_id: &str,
        responses: &[(&str, HttpResponse)], // (cache_key, response)
    ) -> Result<ProductMetadata, ParseError>;

    /// Get HTTP request for search
    fn request_search(&self, query: &str) -> HttpRequest;

    /// Parse search response
    fn parse_search(&self, response: &HttpResponse) -> Result<Vec<SearchResult>, ParseError>;
}

#[derive(Debug)]
pub enum ParseError {
    MissingData(String),
    InvalidFormat(String),
    NetworkError(String),
    Geoblocked(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingData(msg) => write!(f, "Missing data: {}", msg),
            Self::InvalidFormat(msg) => write!(f, "Invalid format: {}", msg),
            Self::NetworkError(msg) => write!(f, "Network error: {}", msg),
            Self::Geoblocked(msg) => write!(f, "Geoblocked: {}", msg),
        }
    }
}
impl std::error::Error for ParseError {}
