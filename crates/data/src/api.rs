//! Data API Implementation
//!
//! Provides the robust, unified data access layer for the application.
//! Handles background fetching, caching, and state management.

use crate::resource::{ResourceManager, ResourceRequest, ResourceType};
use arclain_http::AsyncHttpClient;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

/// Status of a data request
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataStatus {
    Pending,
    Fetching,
    Ready,
    Failed,
    Cached,
}

/// Result of a data polling operation
#[derive(Debug, Clone)]
pub struct DataResult {
    pub status: DataStatus,
    pub data: Option<Vec<u8>>,
    pub error: Option<String>,
}

/// Request for data
#[derive(Debug, Clone)]
pub struct DataRequest {
    pub key: String,
    pub url: String,
    pub resource_type: ResourceType,
    pub product_id: Option<String>,
}

/// Internal state for a pending request
struct PendingDataRequest {
    key: String,
    url: String,
    status: DataStatus,
    data: Option<Vec<u8>>,
    error: Option<String>,
}

/// State container for the Data API
struct DataApiState {
    pending: Mutex<HashMap<String, PendingDataRequest>>,
    next_id: Mutex<u64>,
}

impl DataApiState {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            next_id: Mutex::new(0),
        }
    }

    fn generate_id(&self) -> String {
        let mut id = self.next_id.lock();
        *id += 1;
        format!("data-{}", *id)
    }
}

/// The main Data Service
///
/// Orchestrates ResourceManager and HttpClient to fulfill data requests.
#[derive(Clone)]
pub struct DataService {
    state: Arc<DataApiState>,
    resource_manager: Arc<RwLock<Option<Arc<ResourceManager>>>>,
    http_client: Arc<RwLock<Option<Arc<AsyncHttpClient>>>>,
}

impl DataService {
    pub fn new() -> Self {
        Self {
            state: Arc::new(DataApiState::new()),
            resource_manager: Arc::new(RwLock::new(None)),
            http_client: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_resource_manager(&self, manager: Arc<ResourceManager>) {
        *self.resource_manager.write() = Some(manager);
    }

    pub fn set_http_client(&self, client: Arc<AsyncHttpClient>) {
        *self.http_client.write() = Some(client);
    }

    /// Request data to be fetched/loaded
    /// Returns a request ID
    pub fn request_data(&self, request: DataRequest) -> String {
        let id = self.state.generate_id();
        let key = request.key.clone();
        let url = request.url.clone();

        info!("[DataAPI] Request: {} from {}", key, url);

        // Check if already cached/stored
        if let Some(rm) = self.resource_manager.read().as_ref() {
            if rm.has(&key) {
                debug!("[DataAPI] {} already cached", key);
                self.state.pending.lock().insert(
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

        // Mark as pending
        self.state.pending.lock().insert(
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

    /// Poll for the status of a request
    pub fn poll_data(&self, request_id: &str) -> DataResult {
        let mut pending = self.state.pending.lock();

        if let Some(req) = pending.get_mut(request_id) {
            match req.status {
                DataStatus::Cached => {
                    // Load from resource manager
                    if let Some(rm) = self.resource_manager.read().as_ref() {
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
                        error: Some("Cache miss during load or RM missed".to_string()),
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
                    // Perform fetch
                    let url = req.url.clone();
                    let key = req.key.clone();

                    if let Some(client) = self.http_client.read().as_ref() {
                        info!("[DataAPI] Fetching {} from {}", key, url);

                        match client.blocking_get(&url) {
                            Ok(bytes) => {
                                debug!("[DataAPI] Fetched {} bytes", bytes.len());

                                // Store
                                if let Some(rm) = self.resource_manager.read().as_ref() {
                                    let cache_req = ResourceRequest::from_url(&key, &url)
                                        .with_type(crate::resource::ResourceType::Binary); // TODO: pass correct type?

                                    if let Err(e) = rm.put(&key, &bytes, &cache_req) {
                                        debug!("[DataAPI] Failed to cache: {}", e);
                                    }
                                }

                                req.status = DataStatus::Ready;
                                req.data = Some(bytes.clone());

                                DataResult {
                                    status: DataStatus::Ready,
                                    data: Some(bytes),
                                    error: None,
                                }
                            }
                            Err(e) => {
                                let msg = format!("Fetch failed: {}", e);
                                info!("[DataAPI] {}", msg);
                                req.status = DataStatus::Failed;
                                req.error = Some(msg.clone());
                                DataResult {
                                    status: DataStatus::Failed,
                                    data: None,
                                    error: Some(msg),
                                }
                            }
                        }
                    } else {
                        let msg = "No HTTP client available".to_string();
                        req.status = DataStatus::Failed;
                        req.error = Some(msg.clone());
                        DataResult {
                            status: DataStatus::Failed,
                            data: None,
                            error: Some(msg),
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

    // Check if data exists
    pub fn has_data(&self, key: &str) -> bool {
        if let Some(rm) = self.resource_manager.read().as_ref() {
            rm.has(key)
        } else {
            false
        }
    }

    // Get data directly
    pub fn get_data(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(rm) = self.resource_manager.read().as_ref() {
            rm.get(key).map(|r| r.data)
        } else {
            None
        }
    }
}
