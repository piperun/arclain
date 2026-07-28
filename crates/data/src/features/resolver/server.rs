//! ServerResolver — routes metadata requests through a gameta server API.

use super::{default_materialization_limit, DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::shared::{safe_log_fingerprint, serialize_json_with_limit};
use arclain_network::features::gameta_client::GametaClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Resolver that fetches metadata from a gameta server instance.
///
/// Holds a nullable `Arc<GametaClient>` so it can be registered at
/// startup and reconfigured at runtime. The Arc-wrapped storage lets
/// `try_resolve` snapshot the client under a brief read guard, drop
/// the guard, and run the blocking HTTP call without holding any of
/// the resolver's locks (audit P6 — pre-fix the lock was held
/// throughout the HTTP round-trip, blocking config swaps for the
/// duration).
pub struct ServerResolver {
    client: Arc<RwLock<Option<Arc<GametaClient>>>>,
}

impl ServerResolver {
    /// Create a resolver with no client configured (disabled).
    pub fn new() -> Self {
        Self {
            client: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a resolver pre-configured with a client.
    pub fn with_client(client: GametaClient) -> Self {
        Self {
            client: Arc::new(RwLock::new(Some(Arc::new(client)))),
        }
    }

    /// Replace (or clear) the underlying client at runtime.
    pub fn set_client(&self, client: Option<GametaClient>) {
        *self.client.write() = client.map(Arc::new);
    }

    /// Returns `true` when a client is configured.
    pub fn is_available(&self) -> bool {
        self.client.read().is_some()
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
        self.try_resolve_with_limit(key, _request, default_materialization_limit())
    }

    fn try_resolve_with_limit(
        &self,
        key: &str,
        _request: &DataRequest,
        limit: usize,
    ) -> Result<Vec<u8>, ResolveError> {
        let (source, id) = Self::parse_key(key).ok_or(ResolveError::NotConfigured)?;

        // Snapshot the client Arc under a brief read guard, then drop
        // the guard before the blocking HTTP call. This lets `set_client`
        // and concurrent `try_resolve` calls proceed without waiting
        // for an in-flight request (audit P6).
        let client = self
            .client
            .read()
            .as_ref()
            .cloned()
            .ok_or(ResolveError::NotConfigured)?;

        tracing::debug!(
            "[ServerResolver] Fetching metadata source='{}' id='{}'",
            safe_log_fingerprint(&source),
            safe_log_fingerprint(&id)
        );

        match client.get_metadata_with_limit(&source, &id, limit) {
            Ok(Some(meta)) => {
                tracing::debug!(
                    "[ServerResolver] Got metadata for '{}'",
                    safe_log_fingerprint(&id)
                );
                serialize_json_with_limit(&meta, limit, "server metadata response")
                    .map_err(|error| ResolveError::IoError(error.to_string()))
            }
            Ok(None) => {
                tracing::debug!(
                    "[ServerResolver] Metadata not found for '{}'",
                    safe_log_fingerprint(&id)
                );
                Err(ResolveError::NotFound)
            }
            Err(e) => {
                tracing::warn!(
                    "[ServerResolver] Request failed for '{}': {}",
                    safe_log_fingerprint(&id),
                    safe_log_fingerprint(e.to_string())
                );
                Err(ResolveError::IoError(e))
            }
        }
    }

    /// Check whether the server has metadata for `key` without fetching all
    /// bytes (re-uses `try_resolve` since the server has no cheap HEAD-like
    /// existence check in its current API).
    fn has(&self, key: &str, request: &DataRequest) -> bool {
        self.try_resolve_with_limit(key, request, default_materialization_limit())
            .is_ok()
    }

    fn has_with_limit(&self, key: &str, request: &DataRequest, limit: usize) -> bool {
        self.try_resolve_with_limit(key, request, limit).is_ok()
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

    /// Regression test for P6 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// Pre-fix, `try_resolve` held the resolver's `Mutex<Option<...>>`
    /// guard across the blocking `client.get_metadata` HTTP call.
    /// `set_client` then had to wait for that round-trip to finish
    /// before swapping the client.
    ///
    /// Post-fix, the storage is `RwLock<Option<Arc<GametaClient>>>` and
    /// `try_resolve` clones the inner `Arc` under a brief read guard,
    /// drops the guard, then runs HTTP. This test pins the type-level
    /// shape so a future revert to `Mutex<Option<GametaClient>>`
    /// fails to compile.
    #[test]
    fn p6_server_resolver_storage_is_rwlock_of_arc_client() {
        use arclain_network::features::gameta_client::GametaClient;

        // Type-level assertion: if someone reverts the storage shape,
        // this binding fails to compile.
        fn _accept<T>(_: &Arc<parking_lot::RwLock<Option<Arc<T>>>>) {}
        let resolver = ServerResolver::new();
        _accept::<GametaClient>(&resolver.client);
    }
}
