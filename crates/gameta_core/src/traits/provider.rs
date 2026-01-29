//! Provider trait definition
//!
//! All metadata providers (DLSite, Steam, itch.io, etc.) implement this trait.

use crate::errors::ParseError;
use crate::traits::{HttpRequest, HttpResponse};
use crate::types::{MetadataSource, ProductMetadata, SearchResult};

/// Trait that all metadata providers must implement
///
/// The provider interface uses a two-phase architecture:
/// 1. **Instruction Phase**: Provider returns `Vec<HttpRequest>` describing what to fetch
/// 2. **Response Phase**: Provider parses the responses into metadata
///
/// This decouples provider logic from HTTP execution, allowing:
/// - Different HTTP backends (sync/async, with/without caching)
/// - Testing with mock responses
/// - Request batching and prioritization
pub trait MetadataProvider: Send + Sync {
    /// Provider identifier
    fn id(&self) -> MetadataSource;

    /// Detect product ID from text (e.g., filename, URL)
    ///
    /// Returns the external ID if detected (e.g., "RJ123456" for DLSite)
    fn detect(&self, text: &str) -> Option<String>;

    /// Get HTTP requests needed to fetch metadata
    ///
    /// Returns a list of requests that should be executed.
    /// Each request has a cache_key for matching responses.
    fn request_metadata(&self, external_id: &str) -> Vec<HttpRequest>;

    /// Parse responses into ProductMetadata
    ///
    /// The responses array contains (cache_key, response) pairs.
    /// Provider uses cache_key to identify which response is which.
    fn parse_responses(
        &self,
        external_id: &str,
        responses: &[(&str, HttpResponse)],
    ) -> Result<ProductMetadata, ParseError>;

    /// Get HTTP request for search
    fn request_search(&self, query: &str) -> HttpRequest;

    /// Parse search response
    fn parse_search(&self, response: &HttpResponse) -> Result<Vec<SearchResult>, ParseError>;
}
