//! Network resolver
//!
//! Fetches data from HTTP endpoints.

use super::{DataSourceResolver, ResolveError};
use crate::features::api::DataRequest;
use arclain_http::AsyncHttpClient;
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

        tracing::info!("[NetworkResolver] Fetching from {}", url);

        match self.client.blocking_get(url) {
            Ok(data) => {
                tracing::debug!("[NetworkResolver] Fetched {} bytes", data.len());
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
