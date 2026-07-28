//! DataSourceResolver trait definition

use crate::features::api::DataRequest;
use crate::DEFAULT_MAX_RESOURCE_SIZE_BYTES;

/// Error when a source can't resolve data
#[derive(Debug)]
pub enum ResolveError {
    /// Data not found in this source
    NotFound,
    /// Source not configured (e.g., no URL for network)
    NotConfigured,
    /// I/O or network error
    IoError(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound => write!(f, "Not found"),
            ResolveError::NotConfigured => write!(f, "Not configured"),
            ResolveError::IoError(msg) => write!(f, "I/O error: {}", msg),
        }
    }
}

impl std::error::Error for ResolveError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_error_display_not_found() {
        assert_eq!(ResolveError::NotFound.to_string(), "Not found");
    }

    #[test]
    fn resolve_error_display_not_configured() {
        assert_eq!(ResolveError::NotConfigured.to_string(), "Not configured");
    }

    #[test]
    fn resolve_error_display_io_error() {
        let err = ResolveError::IoError("disk full".into());
        assert_eq!(err.to_string(), "I/O error: disk full");
    }
}

/// Trait for data source resolvers
///
/// Each data source (cache, network, file, etc.) implements this trait.
/// The DataService iterates through a chain of resolvers until one succeeds.
pub trait DataSourceResolver: Send + Sync {
    /// Try to resolve data from this source
    ///
    /// Returns the data bytes if found, or an error if not available.
    fn try_resolve(&self, key: &str, request: &DataRequest) -> Result<Vec<u8>, ResolveError>;

    /// Resolve data under a caller-supplied materialization ceiling.
    ///
    /// Implementations that read, clone, fetch, or serialize a potentially
    /// large body must override this method and enforce `limit` while doing
    /// that work. The default keeps third-party resolver implementations
    /// source-compatible, but can only reject after their legacy method has
    /// returned.
    fn try_resolve_with_limit(
        &self,
        key: &str,
        request: &DataRequest,
        limit: usize,
    ) -> Result<Vec<u8>, ResolveError> {
        let data = self.try_resolve(key, request)?;
        if data.len() > limit {
            return Err(materialized_limit_error(limit));
        }
        Ok(data)
    }

    /// Try to store data to this source (optional)
    ///
    /// Default implementation returns NotConfigured (read-only source).
    fn try_store(
        &self,
        _key: &str,
        _data: &[u8],
        _request: &DataRequest,
    ) -> Result<(), ResolveError> {
        Err(ResolveError::NotConfigured)
    }

    /// Check if this source has data for the key (optional)
    ///
    /// Default implementation tries to resolve and checks for success.
    fn has(&self, key: &str, request: &DataRequest) -> bool {
        self.try_resolve(key, request).is_ok()
    }

    /// Check for a key without allowing a fallback implementation to
    /// materialize more than `limit` bytes.
    ///
    /// Resolvers with a metadata/index lookup should override this method so
    /// existence checks do not read or clone the body at all.
    fn has_with_limit(&self, key: &str, request: &DataRequest, limit: usize) -> bool {
        self.try_resolve_with_limit(key, request, limit).is_ok()
    }
}

pub(crate) fn materialized_limit_error(limit: usize) -> ResolveError {
    ResolveError::IoError(format!(
        "resource exceeds the {limit}-byte materialized read limit"
    ))
}

pub(crate) fn default_materialization_limit() -> usize {
    DEFAULT_MAX_RESOURCE_SIZE_BYTES
}
