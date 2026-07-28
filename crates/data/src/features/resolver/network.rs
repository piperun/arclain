//! Network resolver
//!
//! Fetches data from HTTP endpoints.

use super::{default_materialization_limit, DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use crate::shared::safe_log_fingerprint;
use arclain_network::AsyncHttpClient;
use std::sync::Arc;

/// Resolver for network/HTTP data
pub struct NetworkResolver {
    client: Arc<AsyncHttpClient>,
    plugin_id: Option<String>,
}

impl NetworkResolver {
    pub fn new(client: Arc<AsyncHttpClient>) -> Self {
        Self {
            client,
            plugin_id: None,
        }
    }

    pub fn for_plugin(client: Arc<AsyncHttpClient>, plugin_id: impl Into<String>) -> Self {
        Self {
            client,
            plugin_id: Some(plugin_id.into()),
        }
    }
}

impl DataSourceResolver for NetworkResolver {
    fn try_resolve(&self, _key: &str, request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        self.try_resolve_with_limit(_key, request, default_materialization_limit())
    }

    fn try_resolve_with_limit(
        &self,
        _key: &str,
        request: &DataRequest,
        limit: usize,
    ) -> Result<Vec<u8>, ResolveError> {
        let url = request.url.as_ref().ok_or(ResolveError::NotConfigured)?;

        let result = if let Some(plugin_id) = self.plugin_id.as_deref() {
            tracing::debug!(
                "[NetworkResolver] fetching key='{}' through checked plugin '{}' route",
                safe_log_fingerprint(_key),
                safe_log_fingerprint(plugin_id)
            );
            self.client
                .blocking_get_for_plugin_with_limit(plugin_id, url, limit)
                .map_err(|error| error.to_string())
        } else {
            tracing::debug!(
                "[NetworkResolver] fetching host key='{}' through direct route",
                safe_log_fingerprint(_key)
            );
            self.client.blocking_get_with_limit(url, false, limit)
        };

        match result {
            Ok(data) => {
                tracing::debug!("[NetworkResolver] Fetched {} bytes", data.len());
                // Keep small-response diagnostics without exposing bodies,
                // which may contain credentials or private metadata.
                if data.len() < 500 {
                    tracing::debug!(
                        "[NetworkResolver] SHORT RESPONSE ({} bytes, fingerprint {})",
                        data.len(),
                        safe_log_fingerprint(&data)
                    );
                }
                Ok(data)
            }
            Err(e) => {
                tracing::warn!(
                    "[NetworkResolver] Fetch failed: {}",
                    safe_log_fingerprint(e.to_string())
                );
                Err(ResolveError::IoError(e.to_string()))
            }
        }
    }

    // Network is read-only (no store)
}
