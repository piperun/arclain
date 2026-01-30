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
    /// Client for direct connections (no proxy)
    client_direct: RwLock<reqwest::Client>,
    /// Client for proxied connections (uses ProxyConfig if enabled)
    client_proxied: RwLock<reqwest::Client>,

    /// Map of plugin_id -> use_proxy
    plugin_proxy_map: Arc<RwLock<HashMap<String, bool>>>,

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
        let client_direct = Self::build_client(None);
        let client_proxied = Self::build_client(proxy_config);

        Self {
            client_direct: RwLock::new(client_direct),
            client_proxied: RwLock::new(client_proxied),
            plugin_proxy_map: Arc::new(RwLock::new(HashMap::new())),
            rate_limiter: RwLock::new(RateLimiter::default()),
            whitelist,
            pending: Arc::new(Mutex::new(HashMap::new())),
            runtime,
        }
    }

    /// Update client configuration (e.g. proxy settings)
    pub fn update_config(&self, proxy_config: Option<ProxyConfig>) {
        let new_proxied = Self::build_client(proxy_config);
        *self.client_proxied.write() = new_proxied;
        info!("AsyncHttpClient proxied configuration updated");
    }

    /// Update the plugin proxy map
    pub fn update_plugin_proxy_map(&self, map: HashMap<String, bool>) {
        *self.plugin_proxy_map.write() = map;
    }

    /// Check if a plugin should use the proxy
    pub fn should_use_proxy_for_plugin(&self, plugin_id: &str) -> bool {
        *self
            .plugin_proxy_map
            .read()
            .get(plugin_id)
            .unwrap_or(&false)
    }

    fn build_client(proxy_config: Option<ProxyConfig>) -> reqwest::Client {
        let mut builder = reqwest::Client::builder().user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");

        if let Some(proxy) = proxy_config.and_then(|c| c.to_proxy()) {
            builder = builder.proxy(proxy);
        }

        builder.build().expect("Failed to create HTTP client")
    }

    /// Update the whitelist
    pub fn update_whitelist(&self, whitelist: DomainWhitelist) {
        *self.whitelist.write() = whitelist;
    }

    /// Approve a domain for a plugin
    pub fn approve_domain(&self, plugin_id: &str, domain: &str) {
        self.whitelist.write().approve(plugin_id, domain);
        info!("Auto-approved domain '{}' for plugin '{}'", domain, plugin_id);
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

        // Check whitelist first to allow user-approved deviations (e.g. IPs)
        let whitelist = self.whitelist.read();
        let access = whitelist.check(plugin_id, &domain_info.effective_domain);
        drop(whitelist);

        // If not explicitly allowed, enforce strict security checks
        if access != AccessCheck::Allowed {
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
        }

        match access {
            AccessCheck::Allowed => {}
            AccessCheck::NeedsApproval => {
                return Err(HttpError::DomainNeedsApproval {
                    domain: domain_info.effective_domain,
                });
            }
            AccessCheck::NotWhitelisted => {
                // Add to pending for user approval
                self.whitelist
                    .write()
                    .add_pending(plugin_id, &domain_info.effective_domain);
                return Err(HttpError::DomainNotWhitelisted {
                    domain: domain_info.effective_domain,
                });
            }
        }

        // Check rate limit
        let rate_limiter = self.rate_limiter.read();
        if !rate_limiter.try_acquire(&domain_info.effective_domain) {
            return Err(HttpError::RateLimited {
                domain: domain_info.effective_domain,
            });
        }
        drop(rate_limiter);

        // Determine if proxy should be used
        let use_proxy = *self
            .plugin_proxy_map
            .read()
            .get(plugin_id)
            .unwrap_or(&false);

        // Start the request
        Ok(self.start_request(request, use_proxy))
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

        // Host requests default to direct connection
        self.start_request(request, false)
    }

    /// Internal: start an async request
    fn start_request(&self, request: HttpRequest, use_proxy: bool) -> RequestId {
        let id = RequestId::new();

        // Mark as pending
        self.pending
            .lock()
            .insert(id.clone(), RequestStatus::Pending);

        // Clone what we need for the async task
        let client = if use_proxy {
            self.client_proxied.read().clone()
        } else {
            self.client_direct.read().clone()
        };

        let pending = self.pending.clone();
        let request_id = id.clone();

        debug!(
            "Starting request {} to {} (proxy: {})",
            request_id.0, request.url, use_proxy
        );

        // Spawn async task
        self.runtime.spawn(async move {
            // Update status to in-progress
            pending
                .lock()
                .insert(request_id.clone(), RequestStatus::InProgress);

            // Fix DLSite CDN URLs with padded folder names (from old WASM plugin)
            // e.g., /RJ00361000/ -> /RJ361000/
            let url = if request.url.contains("img.dlsite.jp") {
                fix_dlsite_cdn_folder(&request.url)
            } else {
                request.url.clone()
            };

            // Build request
            let mut req_builder = match request.method {
                HttpMethod::Get => client.get(&url),
                HttpMethod::Post => client.post(&url),
                HttpMethod::Put => client.put(&url),
                HttpMethod::Delete => client.delete(&url),
            };

            // Global/Domain specific headers injection - mimic Firefox exactly
            if request.url.contains("dlsite.com") || request.url.contains("dlsite.jp") {
                info!("Injecting DLSite headers for {}", request.url);
                req_builder = req_builder.header("Cookie", "adultchecked=1; locale=ja-JP");
                req_builder = req_builder.header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                );
                req_builder =
                    req_builder.header("Accept-Language", "ja-JP,ja;q=0.9,en-US;q=0.8,en;q=0.7");
                req_builder = req_builder.header("Accept-Encoding", "gzip, deflate, br");
                req_builder = req_builder.header("Connection", "keep-alive");
                req_builder = req_builder.header("Referer", "https://www.dlsite.com/");
                req_builder = req_builder.header("Sec-Fetch-Dest", "image");
                req_builder = req_builder.header("Sec-Fetch-Mode", "no-cors");
                req_builder = req_builder.header("Sec-Fetch-Site", "cross-site");
                req_builder = req_builder.header("Cache-Control", "no-cache");
                req_builder = req_builder.header("Pragma", "no-cache");
            }

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
    pub fn blocking_get(&self, url: &str, use_proxy: bool) -> Result<Vec<u8>, String> {
        use std::time::Duration;

        let client = if use_proxy {
            self.client_proxied.read().clone()
        } else {
            self.client_direct.read().clone()
        };
        let url = url.to_string();

        // Use the runtime handle to block on the async request
        self.runtime.block_on(async {
            let mut req = client.get(&url);

            // Domain specific headers - mimic Firefox exactly
            if url.contains("dlsite.com") || url.contains("dlsite.jp") {
                info!("Injecting DLSite headers (blocking) for {}", url);
                req = req.header("Cookie", "adultchecked=1; locale=ja-JP");
                req = req.header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                );
                req = req.header("Accept-Language", "ja-JP,ja;q=0.9,en-US;q=0.8,en;q=0.7");
                req = req.header("Accept-Encoding", "gzip, deflate, br");
                req = req.header("Connection", "keep-alive");
                req = req.header("Referer", "https://www.dlsite.com/");
                req = req.header("Sec-Fetch-Dest", "image");
                req = req.header("Sec-Fetch-Mode", "no-cors");
                req = req.header("Sec-Fetch-Site", "cross-site");
                req = req.header("Cache-Control", "no-cache");
                req = req.header("Pragma", "no-cache");
            }

            let response = req
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;

            if !response.status().is_success() {
                return Err(format!("HTTP error: {}", response.status()));
            }

            // Log response headers for debugging (especially Content-Encoding)
            let content_encoding = response
                .headers()
                .get("content-encoding")
                .map(|v| v.to_str().unwrap_or("(invalid)"))
                .unwrap_or("(none)");
            let content_length = response
                .headers()
                .get("content-length")
                .map(|v| v.to_str().unwrap_or("?"))
                .unwrap_or("?");
            tracing::debug!(
                "[HTTP] Response: status={}, content-encoding={}, content-length={}",
                response.status(),
                content_encoding,
                content_length
            );

            response
                .bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| format!("Failed to read body: {}", e))
        })
    }
}

/// Fix DLSite CDN URLs with incorrectly-padded folder names.
/// Folder digit count should match the product ID digit count.
/// e.g., /RJ00361000/RJ360420_ -> /RJ361000/RJ360420_ (6 digits like product)
///       /VJ01006000/VJ01005126_ -> unchanged (8 digits matches product)
fn fix_dlsite_cdn_folder(url: &str) -> String {
    let prefixes = ["RJ", "VJ", "BJ", "RE"];

    for prefix in prefixes {
        // Find the folder pattern (first occurrence of /PREFIX followed by digits and /)
        let folder_pattern = format!("/{}", prefix);
        if let Some(folder_start) = url.find(&folder_pattern) {
            let after_folder_prefix = folder_start + 1 + prefix.len();

            // Find where folder ends (next /)
            if let Some(folder_end_rel) = url[after_folder_prefix..].find('/') {
                let folder_end = after_folder_prefix + folder_end_rel;
                let folder_digits = &url[after_folder_prefix..folder_end];

                // Now find the product ID (second occurrence of PREFIX after folder)
                let after_folder = folder_end + 1;
                if let Some(product_start_rel) = url[after_folder..].find(prefix) {
                    let product_start = after_folder + product_start_rel + prefix.len();

                    // Find where product ID digits end (at _ or .)
                    let product_end = url[product_start..]
                        .find(|c: char| !c.is_ascii_digit())
                        .map(|i| product_start + i)
                        .unwrap_or(url.len());

                    let product_digits = &url[product_start..product_end];

                    // If folder has more digits than product, reformat
                    if folder_digits.len() > product_digits.len()
                        && folder_digits.chars().all(|c| c.is_ascii_digit())
                        && product_digits.chars().all(|c| c.is_ascii_digit())
                    {
                        if let Ok(folder_num) = folder_digits.parse::<u64>() {
                            let target_width = product_digits.len();
                            let fixed_folder_digits =
                                format!("{:0width$}", folder_num, width = target_width);
                            let old_folder = format!("{}{}", prefix, folder_digits);
                            let new_folder = format!("{}{}", prefix, fixed_folder_digits);

                            if old_folder != new_folder {
                                let fixed_url = url.replacen(&old_folder, &new_folder, 1);
                                debug!(
                                    "Fixed DLSite CDN folder: {} -> {}",
                                    old_folder, new_folder
                                );
                                return fixed_url;
                            }
                        }
                    }
                }
            }
        }
    }

    url.to_string()
}
