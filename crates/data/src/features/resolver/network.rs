//! Network resolver
//!
//! Fetches data from HTTP endpoints.

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
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
        let url = request.url.as_ref().ok_or(ResolveError::NotConfigured)?;

        let result = if let Some(plugin_id) = self.plugin_id.as_deref() {
            tracing::debug!(
                "[NetworkResolver] fetching key='{}' through checked plugin '{}' route",
                _key,
                plugin_id
            );
            self.client
                .blocking_get_for_plugin(plugin_id, url)
                .map_err(|error| error.to_string())
        } else {
            tracing::debug!(
                "[NetworkResolver] fetching host key='{}' through direct route",
                _key
            );
            self.client.blocking_get(url, false)
        };

        match result {
            Ok(data) => {
                tracing::debug!("[NetworkResolver] Fetched {} bytes", data.len());
                // DEBUG: Log small responses to diagnose API issues
                if data.len() < 500 {
                    if let Ok(text) = String::from_utf8(data.clone()) {
                        tracing::debug!(
                            "[NetworkResolver] SHORT RESPONSE ({} bytes): {}",
                            data.len(),
                            text
                        );
                    }
                }
                Ok(data)
            }
            Err(e) => {
                tracing::warn!("[NetworkResolver] Fetch failed: {}", e);
                Err(ResolveError::IoError(e.to_string()))
            }
        }
    }

    // Network is read-only (no store)
}
