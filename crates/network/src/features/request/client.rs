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
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Result of a streaming download.
///
/// `bytes_written` is the count of bytes successfully written to the
/// caller's `Write` sink — for a 206 Partial Content response that's
/// the *remaining* bytes from `start_byte`, not the total resource
/// size. `total_size` carries the full resource length when known
/// (from `Content-Range` on 206 or `Content-Length` on 200), so the
/// caller can detect "fully written" without tracking it themselves.
///
/// `was_partial` distinguishes:
/// - `true`: server returned 206; the writer holds bytes from
///   `start_byte` onward and the caller should append.
/// - `false`: server returned 200 (full body); if the caller had
///   prior `.partial` bytes they MUST be discarded — the new body
///   replaces them from byte 0.
#[derive(Debug, Clone)]
pub struct StreamingDownload {
    pub bytes_written: u64,
    pub was_partial: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContentRange {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) total: u64,
}

/// Parse the exact `bytes <start>-<end>/<total>` form used by a 206
/// response. Unknown totals and impossible ranges are not safe for resume.
pub(super) fn parse_content_range(header: &str) -> Option<ContentRange> {
    fn parse_decimal(value: &str) -> Option<u64> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        value.parse().ok()
    }

    let value = header.strip_prefix("bytes ")?;
    let (bounds, total) = value.split_once('/')?;
    let (start, end) = bounds.split_once('-')?;
    let start = parse_decimal(start)?;
    let end = parse_decimal(end)?;
    let total = parse_decimal(total)?;

    (end >= start && total > end).then_some(ContentRange { start, end, total })
}

/// State-carrying completion signal for one asynchronous request.
#[derive(Debug)]
struct CompletionState {
    status: RequestStatus,
    #[cfg(test)]
    status_clone_count: Arc<AtomicUsize>,
}

impl CompletionState {
    fn new(status: RequestStatus) -> Self {
        Self {
            status,
            #[cfg(test)]
            status_clone_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn clone_status(&self) -> RequestStatus {
        #[cfg(test)]
        self.status_clone_count.fetch_add(1, Ordering::Relaxed);
        self.status.clone()
    }
}

impl Clone for CompletionState {
    fn clone(&self) -> Self {
        #[cfg(test)]
        self.status_clone_count.fetch_add(1, Ordering::Relaxed);
        Self {
            status: self.status.clone(),
            #[cfg(test)]
            status_clone_count: self.status_clone_count.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RequestCompletion {
    sender: watch::Sender<CompletionState>,
}

impl RequestCompletion {
    pub(super) fn new(initial: RequestStatus) -> Self {
        let (sender, _receiver) = watch::channel(CompletionState::new(initial));
        Self { sender }
    }

    pub(super) fn status(&self) -> RequestStatus {
        self.sender.borrow().clone_status()
    }

    fn is_complete(&self) -> bool {
        self.sender.borrow().status.is_complete()
    }

    fn is_pending(&self) -> bool {
        self.sender.borrow().status.is_pending()
    }

    #[cfg(test)]
    pub(super) fn status_clone_count(&self) -> usize {
        self.sender
            .borrow()
            .status_clone_count
            .load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn clone_watched_state_for_test(&self) {
        let _cloned: CompletionState = self.sender.borrow().clone();
    }

    pub(super) fn set(&self, status: RequestStatus) {
        self.sender.send_if_modified(move |current| {
            if current.status.is_complete() {
                return false;
            }
            current.status = status;
            true
        });
    }

    pub(super) async fn wait(&self) {
        let mut receiver = self.sender.subscribe();
        loop {
            let is_complete = {
                let status = receiver.borrow_and_update();
                status.status.is_complete()
            };
            if is_complete {
                return;
            }
            receiver
                .changed()
                .await
                .expect("request completion sender lives in PendingEntry");
        }
    }
}

/// One row of the pending-requests map.
#[derive(Debug)]
pub(crate) struct PendingEntry {
    completion: RequestCompletion,
}

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
    pending: Arc<Mutex<HashMap<RequestId, PendingEntry>>>,
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
        let completion = RequestCompletion::new(RequestStatus::Pending);

        // Mark as pending
        self.pending.lock().insert(
            id.clone(),
            PendingEntry {
                completion: completion.clone(),
            },
        );

        // Clone what we need for the async task
        let client = if use_proxy {
            self.client_proxied.read().clone()
        } else {
            self.client_direct.read().clone()
        };

        let request_id = id.clone();

        debug!(
            "Starting request {} to {} (proxy: {})",
            request_id.0, request.url, use_proxy
        );

        // Spawn async task
        self.runtime.spawn(async move {
            // Update status to in-progress
            completion.set(RequestStatus::InProgress);

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
            if is_dlsite_url(&request.url) {
                info!("Injecting DLSite headers for {}", request.url);
                req_builder = inject_dlsite_browser_headers(req_builder);
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

            completion.set(status);
        });

        id
    }

    /// Get the status of a request
    pub fn status(&self, id: &RequestId) -> Option<RequestStatus> {
        self.pending
            .lock()
            .get(id)
            .map(|entry| entry.completion.status())
    }

    /// Await the completion of a request without polling.
    ///
    /// Resolves with `Some(RequestStatus)` once the request reaches a
    /// terminal state (Ready / Failed / Cancelled), or `None` if the
    /// request id is unknown. Compared to a `tokio::time::sleep(...)`
    /// poll loop, this wakes once when the HTTP task finishes
    /// (audit P2).
    pub async fn await_complete(&self, id: &RequestId) -> Option<RequestStatus> {
        let completion = {
            let pending = self.pending.lock();
            let entry = pending.get(id)?;
            entry.completion.clone()
        };

        completion.wait().await;
        self.pending
            .lock()
            .get(id)
            .map(|entry| entry.completion.status())
    }

    /// Take the response (removes from pending)
    pub fn take_response(&self, id: &RequestId) -> Option<RequestStatus> {
        let mut pending = self.pending.lock();
        let is_complete = pending
            .get(id)
            .is_some_and(|entry| entry.completion.is_complete());
        if is_complete {
            return pending.remove(id).map(|entry| entry.completion.status());
        }
        None
    }

    /// Cancel a pending request: notify any waiters, then drop the
    /// entry so it can't accumulate in the `pending` map (audit P19 —
    /// previously `cancel` flipped status to `Cancelled` and left the
    /// entry behind, which leaked memory across long sessions if no
    /// one ever called `take_response` afterwards).
    ///
    /// Subsequent `take_response` / `status` / `await_complete` calls
    /// for the cancelled id return `None` — no caller in the workspace
    /// branched on `RequestStatus::Cancelled` specifically.
    pub fn cancel(&self, id: &RequestId) {
        let mut pending = self.pending.lock();
        if let Some(entry) = pending.get(id) {
            entry.completion.set(RequestStatus::Cancelled);
        }
        pending.remove(id);
    }

    /// Get count of pending requests
    pub fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .values()
            .filter(|entry| entry.completion.is_pending())
            .count()
    }

    /// Total number of entries the `pending` map is currently holding,
    /// including terminal (Ready/Failed) ones that haven't been
    /// `take_response`'d yet. Mainly useful for memory-leak regression
    /// tests; production should use `pending_count` for the
    /// "in-flight" semantic.
    pub fn pending_total(&self) -> usize {
        self.pending.lock().len()
    }

    #[cfg(test)]
    pub(super) fn completion_for_test(&self, id: &RequestId) -> Option<RequestCompletion> {
        self.pending
            .lock()
            .get(id)
            .map(|entry| entry.completion.clone())
    }

    /// Set rate limit for a domain
    pub fn set_rate_limit(&self, domain: &str, requests_per_minute: u32) {
        self.rate_limiter
            .write()
            .set_limit(domain, requests_per_minute);
    }

    /// Streaming GET — fetch a URL into the given writer chunk by chunk,
    /// never holding the full body in host RAM. Returns the number of
    /// bytes written plus the response's ETag and Last-Modified headers
    /// (for resume validation).
    ///
    /// `start_byte`: if `Some(n)`, sends `Range: bytes=n-`. The server's
    /// response status tells us whether the range was honored:
    ///
    /// - 206 Partial Content → server gave us bytes from `n` onward,
    ///   caller appends.
    /// - 200 OK → server returned the full body; caller must truncate
    ///   the partial file and rewrite from byte 0.
    ///
    /// The status is reported via `StreamingDownload::was_partial`.
    ///
    /// `if_match`: if `Some(etag)`, sends `If-Match: <etag>` so the
    /// server returns 412 Precondition Failed when the resource has
    /// changed since the partial download started.
    ///
    /// If a partial response sends more bytes than its declared range,
    /// this writes only the safe prefix through the declared boundary and
    /// returns an error without flushing the caller-owned writer.
    ///
    /// For use from background threads only (uses `block_on`).
    pub fn blocking_get_streaming<W: std::io::Write>(
        &self,
        url: &str,
        use_proxy: bool,
        writer: &mut W,
        start_byte: Option<u64>,
        if_match: Option<&str>,
    ) -> Result<StreamingDownload, String> {
        let client = if use_proxy {
            self.client_proxied.read().clone()
        } else {
            self.client_direct.read().clone()
        };
        let url_owned = url.to_string();
        let start = start_byte;
        let if_match_owned: Option<String> = if_match.map(|s| s.to_string());

        self.runtime.block_on(async move {
            let mut req = client.get(&url_owned);
            if is_dlsite_url(&url_owned) {
                info!("Injecting DLSite headers (streaming) for {}", url_owned);
                req = inject_dlsite_browser_headers(req);
            }
            if let Some(byte) = start {
                req = req.header("Range", format!("bytes={}-", byte));
            }
            if let Some(etag) = if_match_owned.as_deref() {
                req = req.header("If-Match", etag);
            }

            let mut response = req
                .timeout(crate::DEFAULT_REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;

            let status = response.status();
            if !status.is_success() {
                return Err(format!("HTTP error: {}", status));
            }
            // 206 → server honored the range. 200 → server returned full
            // body; caller has to discard any prior partial bytes.
            let was_partial = status.as_u16() == 206;

            // A malformed or mismatched partial response must be rejected
            // before its first body byte reaches the caller's append sink.
            let content_range = if was_partial {
                let requested_start = start.ok_or_else(|| {
                    "Server returned 206 Partial Content without a Range request".to_string()
                })?;
                let header = response
                    .headers()
                    .get("content-range")
                    .ok_or_else(|| "206 response is missing Content-Range".to_string())?
                    .to_str()
                    .map_err(|_| "206 response has an invalid Content-Range header".to_string())?;
                let range = parse_content_range(header).ok_or_else(|| {
                    format!("206 response has an invalid Content-Range: {header}")
                })?;
                if range.start != requested_start {
                    return Err(format!(
                        "206 response starts at byte {}, but byte {} was requested",
                        range.start, requested_start
                    ));
                }

                let declared_length = range.end - range.start + 1;
                if let Some(header) = response.headers().get("content-length") {
                    let content_length = header
                        .to_str()
                        .map_err(|_| {
                            "206 response has an invalid Content-Length header".to_string()
                        })?
                        .parse::<u64>()
                        .map_err(|_| {
                            "206 response has an invalid Content-Length header".to_string()
                        })?;
                    if content_length != declared_length {
                        return Err(format!(
                            "206 Content-Length {content_length} does not match declared range length {declared_length}"
                        ));
                    }
                }

                Some(range)
            } else {
                None
            };

            let etag = response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let last_modified = response
                .headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            // Content-Length is the length of THIS response; for 206 it's
            // the remaining bytes from start_byte, not the resource size.
            // Total size comes from Content-Range when present.
            let total_size = if let Some(range) = content_range {
                Some(range.total)
            } else {
                response
                    .headers()
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
            };

            let declared_length = content_range.map(|range| range.end - range.start + 1);
            let mut total_written: u64 = 0;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| format!("Failed to read body chunk: {}", e))?
            {
                let chunk_length = u64::try_from(chunk.len())
                    .map_err(|_| "Response body chunk is too large to count".to_string())?;
                let next_total = total_written
                    .checked_add(chunk_length)
                    .ok_or_else(|| "Response body byte count overflowed".to_string())?;
                if let Some(declared_length) = declared_length {
                    let remaining = declared_length.checked_sub(total_written).ok_or_else(|| {
                        "Written partial response length exceeded Content-Range".to_string()
                    })?;
                    if chunk_length > remaining {
                        let safe_length = usize::try_from(remaining).map_err(|_| {
                            "Remaining Content-Range length does not fit this platform".to_string()
                        })?;
                        if safe_length > 0 {
                            writer
                                .write_all(&chunk[..safe_length])
                                .map_err(|e| format!("Writer failed: {}", e))?;
                        }
                        return Err(format!(
                            "206 response body exceeds Content-Range: chunk has {chunk_length} bytes with only {remaining} remaining"
                        ));
                    }
                }

                writer
                    .write_all(&chunk)
                    .map_err(|e| format!("Writer failed: {}", e))?;
                total_written = next_total;
            }

            writer
                .flush()
                .map_err(|e| format!("Writer flush failed: {}", e))?;

            if let Some(declared_length) = declared_length {
                if total_written != declared_length {
                    return Err(format!(
                        "206 response body contained {total_written} bytes, but Content-Range declared {declared_length}"
                    ));
                }
            }

            Ok(StreamingDownload {
                bytes_written: total_written,
                was_partial,
                etag,
                last_modified,
                total_size,
            })
        })
    }

    /// Blocking GET request (for use from background threads, NOT main thread)
    /// This uses block_on to wait for the async request to complete.
    pub fn blocking_get(&self, url: &str, use_proxy: bool) -> Result<Vec<u8>, String> {
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
            if is_dlsite_url(&url) {
                info!("Injecting DLSite headers (blocking) for {}", url);
                req = inject_dlsite_browser_headers(req);
            }

            let response = req
                .timeout(crate::DEFAULT_REQUEST_TIMEOUT)
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

/// True if `url` points to dlsite.com or dlsite.jp.
///
/// Used to gate DLsite-specific behavior (browser-mimicking headers,
/// CDN folder-name fix-ups) without sprinkling the literal pair
/// across multiple sites.
pub(crate) fn is_dlsite_url(url: &str) -> bool {
    url.contains("dlsite.com") || url.contains("dlsite.jp")
}

/// Apply the Firefox-mimicking header set DLsite expects on age-gated
/// product/CDN requests. Centralized here so the async and blocking
/// request paths can't drift.
pub(crate) fn inject_dlsite_browser_headers(
    builder: reqwest::RequestBuilder,
) -> reqwest::RequestBuilder {
    builder
        .header("Cookie", "adultchecked=1; locale=ja-JP")
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
        .header("Accept-Language", "ja-JP,ja;q=0.9,en-US;q=0.8,en;q=0.7")
        .header("Accept-Encoding", "gzip, deflate, br")
        .header("Connection", "keep-alive")
        .header("Referer", "https://www.dlsite.com/")
        .header("Sec-Fetch-Dest", "image")
        .header("Sec-Fetch-Mode", "no-cors")
        .header("Sec-Fetch-Site", "cross-site")
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
}

#[cfg(test)]
mod dlsite_url_tests {
    use super::is_dlsite_url;

    #[test]
    fn matches_dlsite_com() {
        assert!(is_dlsite_url("https://www.dlsite.com/maniax/work/=/product_id/RJ123.html"));
    }

    #[test]
    fn matches_dlsite_jp() {
        assert!(is_dlsite_url("https://img.dlsite.jp/modpub/images2/work/.../img.jpg"));
    }

    #[test]
    fn does_not_match_unrelated_host() {
        assert!(!is_dlsite_url("https://example.com/file"));
        assert!(!is_dlsite_url("https://dlsite.example.org/spoof"));
    }
}
