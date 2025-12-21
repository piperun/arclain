//! Data API host function implementations
//!
//! Implements the new unified Data API that replaces scattered HTTP and cache functions.
//! Plugin requests data, host handles network + storage transparently.

use crate::arclain::plugin::host::{DataRequest, DataResult, DataStatus};
use parking_lot::Mutex;
use std::collections::HashMap;
use tracing::{debug, info};

/// Pending async data requests
pub struct PendingDataRequest {
    key: String,
    #[allow(dead_code)]
    url: String,
    status: DataStatus,
    data: Option<Vec<u8>>,
    error: Option<String>,
}

/// Data API state - to be added to HostFunctions

pub struct DataApiState {
    pending: Mutex<HashMap<String, PendingDataRequest>>,
    next_id: Mutex<u64>,
}

impl Default for DataApiState {
    fn default() -> Self {
        Self::new()
    }
}

impl DataApiState {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            next_id: Mutex::new(0),
        }
    }

    pub fn generate_id(&self) -> String {
        let mut id = self.next_id.lock();
        *id += 1;
        format!("data-{}", *id)
    }
}

/// Implementation methods for HostFunctions
impl super::HostFunctions {
    /// Request data - host downloads async, caches transparently
    pub fn impl_request_data(&mut self, request: DataRequest) -> String {
        let id = self.data_api.generate_id();

        let key = request.key.clone();
        let url = request.url.clone();

        info!("[DataAPI] Request: {} from {}", key, url);

        // Check if already cached
        if let Some(rm) = &self.resource_manager {
            if rm.has(&key) {
                debug!("[DataAPI] {} already cached", key);
                self.data_api.pending.lock().insert(
                    id.clone(),
                    PendingDataRequest {
                        key,
                        url,
                        status: DataStatus::Cached,
                        data: None,
                        error: None,
                    },
                );
                return id;
            }
        }

        // Mark as pending - actual fetch will happen in poll
        self.data_api.pending.lock().insert(
            id.clone(),
            PendingDataRequest {
                key,
                url,
                status: DataStatus::Pending,
                data: None,
                error: None,
            },
        );

        id
    }

    /// Poll for data status/result
    pub fn impl_poll_data(&mut self, request_id: String) -> DataResult {
        let mut pending = self.data_api.pending.lock();

        if let Some(req) = pending.get_mut(&request_id) {
            match req.status {
                DataStatus::Cached => {
                    // Load from cache
                    if let Some(rm) = &self.resource_manager {
                        if let Some(resource_data) = rm.get(&req.key) {
                            debug!("[DataAPI] {} loaded from cache", req.key);
                            req.status = DataStatus::Ready;
                            req.data = Some(resource_data.data.clone());
                            return DataResult {
                                status: DataStatus::Ready,
                                data: Some(resource_data.data),
                                error: None,
                            };
                        }
                    }
                    DataResult {
                        status: DataStatus::Failed,
                        data: None,
                        error: Some("Cache miss".to_string()),
                    }
                }
                DataStatus::Ready => DataResult {
                    status: DataStatus::Ready,
                    data: req.data.clone(),
                    error: None,
                },
                DataStatus::Failed => DataResult {
                    status: DataStatus::Failed,
                    data: None,
                    error: req.error.clone(),
                },
                DataStatus::Pending | DataStatus::Fetching => {
                    // Actually perform the fetch NOW (synchronous)
                    let url = req.url.clone();
                    let key = req.key.clone();

                    info!("[DataAPI] Fetching {} from {}", key, url);

                    if let Some(http_client) = &self.async_http_client {
                        // Use blocking fetch
                        match http_client.blocking_get(&url) {
                            Ok(bytes) => {
                                debug!("[DataAPI] Fetched {} bytes for {}", bytes.len(), key);

                                // Cache the result using resource_manager
                                if let Some(rm) = &self.resource_manager {
                                    use arclain_core::features::resource::{
                                        ResourceRequest, ResourceType,
                                    };

                                    let cache_request = ResourceRequest::from_url(&key, &url)
                                        .with_type(ResourceType::Metadata);

                                    if let Err(e) = rm.put(&key, &bytes, &cache_request) {
                                        debug!("[DataAPI] Failed to cache {}: {}", key, e);
                                    } else {
                                        debug!("[DataAPI] Cached {} successfully", key);
                                    }
                                }

                                req.status = DataStatus::Ready;
                                req.data = Some(bytes.clone());

                                return DataResult {
                                    status: DataStatus::Ready,
                                    data: Some(bytes),
                                    error: None,
                                };
                            }
                            Err(e) => {
                                let err_msg = format!("Fetch failed: {}", e);
                                info!("[DataAPI] {}", err_msg);
                                req.status = DataStatus::Failed;
                                req.error = Some(err_msg.clone());

                                return DataResult {
                                    status: DataStatus::Failed,
                                    data: None,
                                    error: Some(err_msg),
                                };
                            }
                        }
                    } else {
                        let err_msg = "No HTTP client available".to_string();
                        req.status = DataStatus::Failed;
                        req.error = Some(err_msg.clone());

                        DataResult {
                            status: DataStatus::Failed,
                            data: None,
                            error: Some(err_msg),
                        }
                    }
                }
            }
        } else {
            DataResult {
                status: DataStatus::Failed,
                data: None,
                error: Some("Unknown request ID".to_string()),
            }
        }
    }

    /// Check if data exists (in cache/storage)
    pub fn impl_has_data(&self, key: String) -> bool {
        if let Some(rm) = &self.resource_manager {
            rm.has(&key)
        } else if let Some(cache) = &self.content_cache {
            cache.has(&key).unwrap_or(false)
        } else {
            false
        }
    }

    /// Get data from storage (no fetch, returns None if not cached)
    pub fn impl_get_data(&self, key: String) -> Option<Vec<u8>> {
        if let Some(rm) = &self.resource_manager {
            rm.get(&key).map(|r| r.data)
        } else if let Some(cache) = &self.content_cache {
            cache.get(&key).ok().flatten()
        } else {
            None
        }
    }
}
