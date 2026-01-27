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
}

impl NetworkResolver {
    pub fn new(client: Arc<AsyncHttpClient>) -> Self {
        Self { client }
    }
}

impl DataSourceResolver for NetworkResolver {
    fn try_resolve(&self, _key: &str, request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        let url = request.url.as_ref().ok_or(ResolveError::NotConfigured)?;

        // Determine if proxy should be used based on plugin_id
        let use_proxy = if let Some(plugin_id) = &request.plugin_id {
            // Check the client's plugin proxy map
            let result = self.client.should_use_proxy_for_plugin(plugin_id);
            tracing::info!(
                "[NetworkResolver] plugin_id='{}' -> use_proxy={}",
                plugin_id,
                result
            );
            result
        } else {
            tracing::info!("[NetworkResolver] No plugin_id provided -> use_proxy=false");
            false
        };

        tracing::info!(
            "[NetworkResolver] Fetching key='{}' url='{}' (proxy: {})",
            _key,
            url,
            use_proxy
        );

        match self.client.blocking_get(url, use_proxy) {
            Ok(data) => {
                tracing::debug!("[NetworkResolver] Fetched {} bytes", data.len());
                // DEBUG: Log small responses to diagnose API issues
                if data.len() < 500 {
                    if let Ok(text) = String::from_utf8(data.clone()) {
                        tracing::info!(
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
