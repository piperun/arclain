//! ServerResolver — routes metadata requests through a gameta server API.

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use arclain_network::features::gameta_client::GametaClient;
use parking_lot::Mutex;
use std::sync::Arc;

/// Resolver that fetches metadata from a gameta server instance.
///
/// Holds a nullable `GametaClient` so it can be registered at startup and
/// reconfigured at runtime without replacing the resolver in the chain.
pub struct ServerResolver {
    client: Arc<Mutex<Option<GametaClient>>>,
}

impl ServerResolver {
    /// Create a resolver with no client configured (disabled).
    pub fn new() -> Self {
        Self {
            client: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a resolver pre-configured with a client.
    pub fn with_client(client: GametaClient) -> Self {
        Self {
            client: Arc::new(Mutex::new(Some(client))),
        }
    }

    /// Replace (or clear) the underlying client at runtime.
    pub fn set_client(&self, client: Option<GametaClient>) {
        *self.client.lock() = client;
    }

    /// Returns `true` when a client is configured.
    pub fn is_available(&self) -> bool {
        self.client.lock().is_some()
    }

    /// Parse a data key into `(source, id)` for the gameta API.
    ///
    /// Recognised formats:
    /// - `"dlsite:json:RJ123456"` → `("dlsite", "RJ123456")`
    /// - `"RJ123456"` / `"RE012345"` / `"VJ000001"` (bare DLSite IDs) →
    ///   `("dlsite", "<id>")`
    ///
    /// Returns `None` for any key that doesn't look like a metadata key we
    /// should forward to the server.
    fn parse_key(key: &str) -> Option<(String, String)> {
        // Explicit "dlsite:json:<id>" form
        if let Some(id) = key.strip_prefix("dlsite:json:") {
            return Some(("dlsite".to_string(), id.to_string()));
        }

        // Bare IDs that start with a DLSite prefix letter-pair
        let bare = key;
        let looks_like_dlsite = bare.len() >= 2 && {
            let prefix = &bare[..2].to_ascii_uppercase();
            matches!(prefix.as_str(), "RJ" | "RE" | "VJ" | "BJ")
        };
        if looks_like_dlsite {
            return Some(("dlsite".to_string(), bare.to_string()));
        }

        None
    }
}

impl Default for ServerResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSourceResolver for ServerResolver {
    fn try_resolve(&self, key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        let (source, id) = Self::parse_key(key).ok_or(ResolveError::NotConfigured)?;

        let guard = self.client.lock();
        let client = guard.as_ref().ok_or(ResolveError::NotConfigured)?;

        tracing::debug!(
            "[ServerResolver] Fetching metadata source='{}' id='{}'",
            source,
            id
        );

        match client.get_metadata(&source, &id) {
            Ok(Some(meta)) => {
                tracing::debug!("[ServerResolver] Got metadata for '{}'", id);
                serde_json::to_vec(&meta)
                    .map_err(|e| ResolveError::IoError(format!("Serialize error: {}", e)))
            }
            Ok(None) => {
                tracing::debug!("[ServerResolver] Metadata not found for '{}'", id);
                Err(ResolveError::NotFound)
            }
            Err(e) => {
                tracing::warn!("[ServerResolver] Request failed for '{}': {}", id, e);
                Err(ResolveError::IoError(e))
            }
        }
    }

    /// Check whether the server has metadata for `key` without fetching all
    /// bytes (re-uses `try_resolve` since the server has no cheap HEAD-like
    /// existence check in its current API).
    fn has(&self, key: &str, request: &DataRequest) -> bool {
        self.try_resolve(key, request).is_ok()
    }

    // `try_store` intentionally inherits the default no-op: the server is
    // read-only from the client's perspective.
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_request() -> DataRequest {
        DataRequest::new("dummy")
    }

    // --- parse_key ---

    #[test]
    fn parse_key_dlsite_json_prefix() {
        let result = ServerResolver::parse_key("dlsite:json:RJ123456");
        assert_eq!(result, Some(("dlsite".to_string(), "RJ123456".to_string())));
    }

    #[test]
    fn parse_key_bare_rj_id() {
        let result = ServerResolver::parse_key("RJ654321");
        assert_eq!(result, Some(("dlsite".to_string(), "RJ654321".to_string())));
    }

    #[test]
    fn parse_key_bare_re_id() {
        let result = ServerResolver::parse_key("RE000001");
        assert_eq!(result, Some(("dlsite".to_string(), "RE000001".to_string())));
    }

    #[test]
    fn parse_key_bare_vj_id() {
        let result = ServerResolver::parse_key("VJ012345");
        assert_eq!(result, Some(("dlsite".to_string(), "VJ012345".to_string())));
    }

    #[test]
    fn parse_key_bare_bj_id() {
        let result = ServerResolver::parse_key("BJ001234");
        assert_eq!(result, Some(("dlsite".to_string(), "BJ001234".to_string())));
    }

    #[test]
    fn parse_key_unknown_format_returns_none() {
        assert!(ServerResolver::parse_key("some:unknown:key").is_none());
        assert!(ServerResolver::parse_key("dlsite:html:RJ123456").is_none());
        assert!(ServerResolver::parse_key("image/jpeg").is_none());
    }

    #[test]
    fn parse_key_steam_key_returns_none() {
        // Non-DLSite sources should not be routed through this resolver
        assert!(ServerResolver::parse_key("steam:12345").is_none());
    }

    #[test]
    fn parse_key_short_string_returns_none() {
        // Single character or empty strings must not panic
        assert!(ServerResolver::parse_key("R").is_none());
        assert!(ServerResolver::parse_key("").is_none());
    }

    // --- is_available ---

    #[test]
    fn is_available_false_when_no_client() {
        let resolver = ServerResolver::new();
        assert!(!resolver.is_available());
    }

    // --- try_resolve without a client ---

    #[test]
    fn try_resolve_returns_not_configured_when_no_client() {
        let resolver = ServerResolver::new();
        let req = dummy_request();
        let result = resolver.try_resolve("dlsite:json:RJ999999", &req);
        assert!(matches!(result, Err(ResolveError::NotConfigured)));
    }

    #[test]
    fn try_resolve_returns_not_configured_for_non_metadata_key() {
        let resolver = ServerResolver::new();
        let req = dummy_request();
        // Even with a client, an unrecognised key should be NotConfigured
        let result = resolver.try_resolve("cache:image:thumbnail.jpg", &req);
        assert!(matches!(result, Err(ResolveError::NotConfigured)));
    }

    // --- try_store is a no-op ---

    #[test]
    fn try_store_returns_not_configured() {
        let resolver = ServerResolver::new();
        let req = dummy_request();
        let result = resolver.try_store("dlsite:json:RJ123456", b"data", &req);
        assert!(matches!(result, Err(ResolveError::NotConfigured)));
    }

    // --- set_client ---

    #[test]
    fn set_client_none_disables_resolver() {
        use arclain_network::features::gameta_client::{GametaClient, ServerConfig};
        let client = GametaClient::new(ServerConfig {
            url: "http://localhost:8080".to_string(),
            api_key: None,
        });
        let resolver = ServerResolver::with_client(client);
        assert!(resolver.is_available());

        resolver.set_client(None);
        assert!(!resolver.is_available());
    }
}
