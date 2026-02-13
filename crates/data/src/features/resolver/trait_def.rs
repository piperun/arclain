//! DataSourceResolver trait definition

use crate::features::api::DataRequest;

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
}
