//! Async HTTP client
//!
//! Non-blocking HTTP client that manages requests asynchronously.

use super::types::{HttpRequest, RequestId, RequestStatus};
use crate::features::rate_limiting::RateLimiter;
use crate::features::security::{analyze_url, DomainInfo};
use crate::features::whitelist::{AccessCheck, DomainWhitelist};
use crate::shared::{HttpError, HttpMethod, HttpResponse};

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::{debug, info, warn};

/// Async HTTP client with security features
pub struct AsyncHttpClient {
    /// Inner reqwest client (wrapped in RwLock for dynamic reconfiguration)
    inner: RwLock<reqwest::Client>,
    /// Rate limiter
    rate_limiter: RwLock<RateLimiter>,
    /// Domain whitelist
    whitelist: Arc<RwLock<DomainWhitelist>>,
    /// Pending requests
    pending: Arc<Mutex<HashMap<RequestId, RequestStatus>>>,
    /// Tokio runtime handle
    runtime: Handle,
}

use crate::features::proxy::ProxyConfig;

impl AsyncHttpClient {
    /// Create a new client with the given whitelist
    pub fn new(
        runtime: Handle,
        whitelist: Arc<RwLock<DomainWhitelist>>,
        proxy_config: Option<ProxyConfig>,
    ) -> Self {
        let client = Self::build_client(proxy_config);

        Self {
            inner: RwLock::new(client),
            rate_limiter: RwLock::new(RateLimiter::default()),
            whitelist,
            pending: Arc::new(Mutex::new(HashMap::new())),
            runtime,
        }
    }

    /// Update client configuration (e.g. proxy settings)
    pub fn update_config(&self, proxy_config: Option<ProxyConfig>) {
        let new_client = Self::build_client(proxy_config);
        *self.inner.write() = new_client;
        info!("AsyncHttpClient configuration updated");
    }

    fn build_client(proxy_config: Option<ProxyConfig>) -> reqwest::Client {
        let mut builder = reqwest::Client::builder().user_agent("Arclain/1.0");

        if let Some(proxy) = proxy_config.and_then(|c| c.to_proxy()) {
            builder = builder.proxy(proxy);
        }

        builder.build().expect("Failed to create HTTP client")
    }

    /// Update the whitelist
    pub fn update_whitelist(&self, whitelist: DomainWhitelist) {
        *self.whitelist.write() = whitelist;
    }

    /// Analyze a URL for security issues (without making a request)
    pub fn analyze_url(&self, url: &str) -> Result<DomainInfo, String> {
        analyze_url(url)
    }

    /// Start a request for a plugin (with whitelist and rate limit checks)
    pub fn request_for_plugin(
        &self,
        plugin_id: &str,
        request: HttpRequest,
    ) -> Result<RequestId, HttpError> {
        // Analyze URL
        let domain_info =
            analyze_url(&request.url).map_err(|e| HttpError::InvalidUrl { reason: e })?;

        // Check for critical security warnings
        if domain_info.has_critical_warnings() {
            let warnings: Vec<String> = domain_info
                .warnings
                .iter()
                .map(|w| w.description())
                .collect();
            return Err(HttpError::SecurityWarning {
                message: warnings.join("; "),
            });
        }

        // Check whitelist
        let whitelist = self.whitelist.read();
        match whitelist.check(plugin_id, &domain_info.effective_domain) {
            AccessCheck::Allowed => {}
            AccessCheck::NeedsApproval => {
                return Err(HttpError::DomainNeedsApproval {
                    domain: domain_info.effective_domain,
                });
            }
            AccessCheck::NotWhitelisted => {
                // Add to pending for user approval
                drop(whitelist);
                self.whitelist
                    .write()
                    .add_pending(plugin_id, &domain_info.effective_domain);
                return Err(HttpError::DomainNotWhitelisted {
                    domain: domain_info.effective_domain,
                });
            }
        }
        drop(whitelist);

        // Check rate limit
        let rate_limiter = self.rate_limiter.read();
        if !rate_limiter.try_acquire(&domain_info.effective_domain) {
            return Err(HttpError::RateLimited {
                domain: domain_info.effective_domain,
            });
        }
        drop(rate_limiter);

        // Start the request
        Ok(self.start_request(request))
    }

    /// Start a request without plugin restrictions (for host use)
    pub fn request(&self, request: HttpRequest) -> RequestId {
        // Check rate limit (still applies for courtesy)
        if let Ok(domain_info) = analyze_url(&request.url) {
            let rate_limiter = self.rate_limiter.read();
            if !rate_limiter.try_acquire(&domain_info.effective_domain) {
                warn!("Rate limit exceeded for {}", domain_info.effective_domain);
            }
        }

        self.start_request(request)
    }

    /// Internal: start an async request
    fn start_request(&self, request: HttpRequest) -> RequestId {
        let id = RequestId::new();

        // Mark as pending
        self.pending
            .lock()
            .insert(id.clone(), RequestStatus::Pending);

        // Clone what we need for the async task
        let client = self.inner.read().clone();
        let pending = self.pending.clone();
        let request_id = id.clone();

        debug!("Starting request {} to {}", request_id.0, request.url);

        // Spawn async task
        self.runtime.spawn(async move {
            // Update status to in-progress
            pending
                .lock()
                .insert(request_id.clone(), RequestStatus::InProgress);

            // Build request
            let mut req_builder = match request.method {
                HttpMethod::Get => client.get(&request.url),
                HttpMethod::Post => client.post(&request.url),
                HttpMethod::Put => client.put(&request.url),
                HttpMethod::Delete => client.delete(&request.url),
            };

            // Add headers
            for (key, value) in &request.headers {
                req_builder = req_builder.header(key, value);
            }

            // Add body
            if let Some(body) = request.body {
                req_builder = req_builder.body(body);
            }

            // Set timeout
            req_builder = req_builder.timeout(request.timeout);

            // Execute
            let result = req_builder.send().await;

            let status = match result {
                Ok(response) => {
                    let status_code = response.status().as_u16();
                    let headers: HashMap<String, String> = response
                        .headers()
                        .iter()
                        .filter_map(|(k, v)| {
                            v.to_str().ok().map(|v| (k.to_string(), v.to_string()))
                        })
                        .collect();
                    let content_type = headers.get("content-type").cloned();

                    match response.bytes().await {
                        Ok(body) => {
                            info!("Request {} completed: {} bytes", request_id.0, body.len());
                            RequestStatus::Ready(HttpResponse {
                                status_code,
                                headers,
                                body: body.to_vec(),
                                content_type,
                            })
                        }
                        Err(e) => {
                            warn!("Request {} body error: {}", request_id.0, e);
                            RequestStatus::Failed(format!("Failed to read body: {}", e))
                        }
                    }
                }
                Err(e) => {
                    warn!("Request {} failed: {}", request_id.0, e);
                    if e.is_timeout() {
                        RequestStatus::Failed("Request timed out".to_string())
                    } else {
                        RequestStatus::Failed(e.to_string())
                    }
                }
            };

            pending.lock().insert(request_id, status);
        });

        id
    }

    /// Get the status of a request
    pub fn status(&self, id: &RequestId) -> Option<RequestStatus> {
        self.pending.lock().get(id).cloned()
    }

    /// Take the response (removes from pending)
    pub fn take_response(&self, id: &RequestId) -> Option<RequestStatus> {
        let mut pending = self.pending.lock();
        if let Some(status) = pending.get(id) {
            if status.is_complete() {
                return pending.remove(id);
            }
        }
        None
    }

    /// Cancel a pending request
    pub fn cancel(&self, id: &RequestId) {
        self.pending
            .lock()
            .insert(id.clone(), RequestStatus::Cancelled);
    }

    /// Get count of pending requests
    pub fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .values()
            .filter(|s| s.is_pending())
            .count()
    }

    /// Set rate limit for a domain
    pub fn set_rate_limit(&self, domain: &str, requests_per_minute: u32) {
        self.rate_limiter
            .write()
            .set_limit(domain, requests_per_minute);
    }

    /// Blocking GET request (for use from background threads, NOT main thread)
    /// This uses block_on to wait for the async request to complete.
    pub fn blocking_get(&self, url: &str) -> Result<Vec<u8>, String> {
        use std::time::Duration;

        let client = self.inner.read().clone();
        let url = url.to_string();

        // Use the runtime handle to block on the async request
        self.runtime.block_on(async {
            let response = client
                .get(&url)
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;

            if !response.status().is_success() {
                return Err(format!("HTTP error: {}", response.status()));
            }

            response
                .bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| format!("Failed to read body: {}", e))
        })
    }
}
