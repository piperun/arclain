//! Async HTTP client
//!
//! Non-blocking HTTP client that manages requests asynchronously.

use super::plugin_policy::{
    validate_plugin_headers, validate_plugin_url, validate_redirect_target,
    validate_resolved_addresses, AuthorizedPluginTarget, MAX_PLUGIN_REDIRECTS,
};
use super::types::{HttpRequest, RequestId, RequestStatus};
use super::PluginNetworkPolicy;
use crate::features::rate_limiting::RateLimiter;
use crate::features::security::{analyze_url, DomainInfo};
use crate::features::whitelist::{AccessCheck, DomainWhitelist};
use crate::shared::{HttpError, HttpMethod, HttpResponse};

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
#[cfg(test)]
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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

/// Validated response identity delivered before any streaming body bytes.
///
/// Callers can use this callback boundary to atomically persist the exact
/// representation they are about to write. `validated_url` is the final URL
/// after redirects, while the range fields describe the response body that
/// will follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingResponseMetadata {
    pub validated_url: String,
    pub was_partial: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub range_start: Option<u64>,
    pub total_size: Option<u64>,
    pub expected_body_length: Option<u64>,
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

struct ProxyRuntimeState {
    proxy_config: Option<ProxyConfig>,
    plugin_proxy_map: HashMap<String, bool>,
    client_proxied: reqwest::Client,
    plugin_routing_available: bool,
}

impl ProxyRuntimeState {
    fn new(proxy_config: Option<ProxyConfig>, plugin_proxy_map: HashMap<String, bool>) -> Self {
        let client_proxied = AsyncHttpClient::build_client(proxy_config.clone());
        Self {
            proxy_config,
            plugin_proxy_map,
            client_proxied,
            plugin_routing_available: true,
        }
    }

    fn unavailable() -> Self {
        Self {
            proxy_config: None,
            plugin_proxy_map: HashMap::new(),
            client_proxied: AsyncHttpClient::build_client(None),
            plugin_routing_available: false,
        }
    }
}

struct PluginRequestContext {
    proxy_runtime: RwLock<ProxyRuntimeState>,
    plugin_policies: RwLock<HashMap<String, PluginNetworkPolicy>>,
    rate_limiter: RwLock<RateLimiter>,
    whitelist: Arc<RwLock<DomainWhitelist>>,
    #[cfg(test)]
    allow_special_addresses: AtomicBool,
    #[cfg(test)]
    dns_answers: RwLock<HashMap<String, VecDeque<Vec<SocketAddr>>>>,
    #[cfg(test)]
    dns_lookup_counts: Mutex<HashMap<String, usize>>,
}

impl PluginRequestContext {
    fn registered_policy(&self, plugin_id: &str) -> Result<PluginNetworkPolicy, HttpError> {
        if self
            .proxy_runtime
            .try_read()
            .is_some_and(|runtime| !runtime.plugin_routing_available)
        {
            return Err(HttpError::RequestFailed {
                message: "plugin network routing is unavailable".to_string(),
            });
        }
        let policy = self
            .plugin_policies
            .read()
            .get(plugin_id)
            .copied()
            .ok_or_else(|| HttpError::PluginNetworkNotConfigured {
                plugin_id: plugin_id.to_string(),
            })?;
        if !policy.network_enabled {
            return Err(HttpError::PluginNetworkDisabled {
                plugin_id: plugin_id.to_string(),
            });
        }
        Ok(policy)
    }

    async fn resolve_host(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, HttpError> {
        #[cfg(test)]
        {
            let normalized_host = host.to_ascii_lowercase();
            if self.dns_answers.read().contains_key(&normalized_host) {
                *self
                    .dns_lookup_counts
                    .lock()
                    .entry(normalized_host.clone())
                    .or_default() += 1;
                let mut answers = self.dns_answers.write();
                let sequence = answers
                    .get_mut(&normalized_host)
                    .expect("checked test DNS entry disappeared");
                let mut resolved = if sequence.len() > 1 {
                    sequence.pop_front().expect("non-empty test DNS sequence")
                } else {
                    sequence.front().cloned().unwrap_or_default()
                };
                for address in &mut resolved {
                    address.set_port(port);
                }
                return Ok(resolved);
            }
        }

        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.collect())
            .map_err(|error| HttpError::DnsResolutionFailed {
                host: host.to_string(),
                reason: error.to_string(),
            })
    }

    async fn authorize_target(
        &self,
        plugin_id: &str,
        url: url::Url,
    ) -> Result<AuthorizedPluginTarget, HttpError> {
        let policy = self.registered_policy(plugin_id)?;
        let url = validate_plugin_url(url.as_str())?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| HttpError::InvalidUrl {
                reason: "plugin URL has no usable port".to_string(),
            })?;
        let (host, resolved) = match url.host().ok_or_else(|| HttpError::InvalidUrl {
            reason: "plugin URL has no host".to_string(),
        })? {
            url::Host::Domain(domain) => {
                let host = domain.to_ascii_lowercase();
                let resolved = self.resolve_host(&host, port).await?;
                (host, resolved)
            }
            url::Host::Ipv4(address) => {
                let host = address.to_string();
                (host, vec![SocketAddr::new(IpAddr::V4(address), port)])
            }
            url::Host::Ipv6(address) => {
                let host = address.to_string();
                (host, vec![SocketAddr::new(IpAddr::V6(address), port)])
            }
        };
        let resolved_ips: Vec<IpAddr> = resolved.iter().map(|address| address.ip()).collect();
        #[cfg(test)]
        let allow_special = self.allow_special_addresses.load(Ordering::Relaxed);
        #[cfg(not(test))]
        let allow_special = false;
        if !allow_special {
            validate_resolved_addresses(&resolved_ips)?;
        } else if resolved.is_empty() {
            return Err(HttpError::DnsResolutionFailed {
                host,
                reason: "resolver returned no addresses".to_string(),
            });
        }

        let rate_domain = if matches!(url.host(), Some(url::Host::Domain(_))) {
            analyze_url(url.as_str())
                .map_err(|reason| HttpError::InvalidUrl { reason })?
                .effective_domain
        } else {
            host.clone()
        };
        let access = self.whitelist.read().check(plugin_id, &host);
        match access {
            AccessCheck::Allowed => {}
            AccessCheck::NeedsApproval => {
                return Err(HttpError::DomainNeedsApproval { domain: host });
            }
            AccessCheck::NotWhitelisted => {
                self.whitelist.write().add_pending(plugin_id, &host);
                return Err(HttpError::DomainNotWhitelisted { domain: host });
            }
        }

        let scope = format!("{plugin_id}\0{rate_domain}");
        if !self
            .rate_limiter
            .read()
            .try_acquire_with_limit(&scope, policy.requests_per_minute)
        {
            return Err(HttpError::RateLimited {
                domain: rate_domain,
            });
        }

        let proxy_config = {
            let runtime = self.proxy_runtime.read();
            if !runtime.plugin_routing_available {
                return Err(HttpError::RequestFailed {
                    message: "plugin network routing is unavailable".to_string(),
                });
            }
            if *runtime.plugin_proxy_map.get(plugin_id).unwrap_or(&false) {
                Some(
                    runtime
                        .proxy_config
                        .as_ref()
                        .filter(|config| config.enabled)
                        .cloned()
                        .ok_or_else(|| HttpError::RequestFailed {
                            message: "plugin is configured to use a proxy, but no enabled proxy is configured"
                                .to_string(),
                        })?,
                )
            } else {
                None
            }
        };
        Ok(AuthorizedPluginTarget {
            url,
            proxy_config,
            resolved,
        })
    }

    fn build_pinned_client(
        &self,
        target: &AuthorizedPluginTarget,
    ) -> Result<reqwest::Client, HttpError> {
        let host = match target.url.host().ok_or_else(|| HttpError::InvalidUrl {
            reason: "plugin URL has no host".to_string(),
        })? {
            url::Host::Domain(domain) => domain.to_ascii_lowercase(),
            url::Host::Ipv4(address) => address.to_string(),
            url::Host::Ipv6(address) => address.to_string(),
        };
        let proxy_resolved = target.proxy_config.is_some().then(|| {
            target
                .resolved
                .iter()
                .copied()
                .filter(SocketAddr::is_ipv4)
                .collect::<Vec<_>>()
        });
        let pinned_addresses = match proxy_resolved.as_deref() {
            Some([]) => {
                // reqwest 0.12.28 formats a locally resolved IPv6 SOCKS target
                // as a bracketed domain, causing hyper-util to emit hostname
                // ATYP instead of IPv6 ATYP. Fail closed until the locked
                // transport can represent the already-validated address.
                return Err(HttpError::PinnedResolutionUnavailable {
                    reason: "the locked SOCKS transport cannot encode a validated IPv6 target as an IP destination"
                        .to_string(),
                });
            }
            Some(addresses) => addresses,
            None => target.resolved.as_slice(),
        };
        let mut builder =
            AsyncHttpClient::client_builder().resolve_to_addrs(&host, pinned_addresses);

        if let Some(config) = target.proxy_config.as_ref() {
            let proxy = config
                .create_pinned_proxy()
                .map_err(|message| HttpError::RequestFailed { message })?;
            builder = builder.proxy(proxy);
        }

        builder.build().map_err(|error| HttpError::RequestFailed {
            message: format!("failed to build pinned plugin HTTP client: {error}"),
        })
    }

    async fn execute(
        &self,
        plugin_id: &str,
        request: HttpRequest,
    ) -> Result<reqwest::Response, HttpError> {
        validate_plugin_headers(&request.headers)?;
        let initial_url = fix_dlsite_cdn_folder(&request.url);
        let mut url = validate_plugin_url(&initial_url)?;
        let mut method = request.method;
        let mut headers = request.headers;
        let mut body = request.body;
        let mut redirects_followed = 0;

        loop {
            let target = self.authorize_target(plugin_id, url).await?;
            let client = self.build_pinned_client(&target)?;
            let mut builder = match method {
                HttpMethod::Get => client.get(target.url.clone()),
                HttpMethod::Post => client.post(target.url.clone()),
                HttpMethod::Put => client.put(target.url.clone()),
                HttpMethod::Delete => client.delete(target.url.clone()),
            };
            if is_dlsite_url(target.url.as_str()) {
                builder = inject_dlsite_browser_headers(builder);
            }
            for (name, value) in &headers {
                builder = builder.header(name, value);
            }
            if let Some(request_body) = body.clone() {
                builder = builder.body(request_body);
            }

            let response = builder
                .timeout(request.timeout)
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        HttpError::Timeout
                    } else {
                        HttpError::RequestFailed {
                            message: error.to_string(),
                        }
                    }
                })?;

            if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                return Ok(response);
            };
            if redirects_followed >= MAX_PLUGIN_REDIRECTS {
                return Err(HttpError::RedirectLimitExceeded);
            }
            let location = location.to_str().map_err(|_| HttpError::InvalidUrl {
                reason: "redirect Location is not valid text".to_string(),
            })?;
            let next_url = validate_redirect_target(&target.url, location)?;

            if target.url.origin() != next_url.origin() {
                headers.retain(|name, _| {
                    !name.eq_ignore_ascii_case("authorization")
                        && !name.eq_ignore_ascii_case("cookie")
                });
            }
            if response.status().as_u16() == 303
                || (matches!(response.status().as_u16(), 301 | 302) && method == HttpMethod::Post)
            {
                method = HttpMethod::Get;
                body = None;
                headers.retain(|name, _| {
                    !name.eq_ignore_ascii_case("content-length")
                        && !name.eq_ignore_ascii_case("content-type")
                        && !name.eq_ignore_ascii_case("transfer-encoding")
                });
            }

            redirects_followed += 1;
            url = next_url;
        }
    }
}

/// Async HTTP client with security features
pub struct AsyncHttpClient {
    /// Client for direct connections (no proxy)
    client_direct: reqwest::Client,

    /// Checked request policy plus the atomically applied proxy runtime.
    plugin_context: Arc<PluginRequestContext>,
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

        Self {
            client_direct,
            plugin_context: Arc::new(PluginRequestContext {
                proxy_runtime: RwLock::new(ProxyRuntimeState::new(proxy_config, HashMap::new())),
                plugin_policies: RwLock::new(HashMap::new()),
                rate_limiter: RwLock::new(RateLimiter::default()),
                whitelist,
                #[cfg(test)]
                allow_special_addresses: AtomicBool::new(false),
                #[cfg(test)]
                dns_answers: RwLock::new(HashMap::new()),
                #[cfg(test)]
                dns_lookup_counts: Mutex::new(HashMap::new()),
            }),
            pending: Arc::new(Mutex::new(HashMap::new())),
            runtime,
        }
    }

    /// Atomically replace the proxy transport and effective plugin routing.
    ///
    /// Building the derived host client happens before the write lock is
    /// acquired. Readers therefore observe either the complete previous state
    /// or the complete replacement, never a new transport with an old route
    /// map (or vice versa).
    pub fn apply_proxy_routing(
        &self,
        proxy_config: Option<ProxyConfig>,
        plugin_proxy_map: HashMap<String, bool>,
    ) {
        let replacement = ProxyRuntimeState::new(proxy_config, plugin_proxy_map);
        *self.plugin_context.proxy_runtime.write() = replacement;
        info!("AsyncHttpClient proxy routing updated");
    }

    /// Atomically deny checked plugin requests when persisted proxy routing
    /// cannot be resolved safely. Host-owned direct requests remain usable,
    /// and a later successful [`Self::apply_proxy_routing`] clears this state.
    pub fn mark_plugin_routing_unavailable(&self) {
        *self.plugin_context.proxy_runtime.write() = ProxyRuntimeState::unavailable();
        warn!("AsyncHttpClient plugin routing marked unavailable");
    }

    /// Register the network capability and request budget for a plugin.
    pub fn configure_plugin(&self, plugin_id: &str, policy: PluginNetworkPolicy) {
        self.plugin_context
            .plugin_policies
            .write()
            .insert(plugin_id.to_string(), policy);
    }

    /// Return the registered manifest-derived policy for observability and
    /// tests. Request authorization still happens inside the checked executor.
    pub fn plugin_network_policy(&self, plugin_id: &str) -> Option<PluginNetworkPolicy> {
        self.plugin_context
            .plugin_policies
            .read()
            .get(plugin_id)
            .copied()
    }

    #[cfg(test)]
    pub(super) fn allow_special_plugin_addresses_for_test(&self) {
        self.plugin_context
            .allow_special_addresses
            .store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn set_plugin_dns_answers_for_test(
        &self,
        host: &str,
        answers: Vec<Vec<SocketAddr>>,
    ) {
        self.plugin_context
            .dns_answers
            .write()
            .insert(host.to_ascii_lowercase(), answers.into_iter().collect());
    }

    #[cfg(test)]
    pub(super) fn plugin_dns_lookup_count_for_test(&self, host: &str) -> usize {
        self.plugin_context
            .dns_lookup_counts
            .lock()
            .get(&host.to_ascii_lowercase())
            .copied()
            .unwrap_or(0)
    }

    /// Atomically replace only per-plugin routing while retaining transport.
    pub fn apply_plugin_proxy_map(&self, map: HashMap<String, bool>) {
        self.plugin_context.proxy_runtime.write().plugin_proxy_map = map;
    }

    /// Check if a plugin should use the proxy
    pub fn should_use_proxy_for_plugin(&self, plugin_id: &str) -> bool {
        *self
            .plugin_context
            .proxy_runtime
            .read()
            .plugin_proxy_map
            .get(plugin_id)
            .unwrap_or(&false)
    }

    fn build_client(proxy_config: Option<ProxyConfig>) -> reqwest::Client {
        let mut builder = Self::client_builder();

        if let Some(proxy) = proxy_config.and_then(|c| c.to_proxy()) {
            builder = builder.proxy(proxy);
        }

        builder.build().expect("Failed to create HTTP client")
    }

    fn client_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
    }

    #[cfg(test)]
    pub(super) fn build_pinned_plugin_client(
        &self,
        target: &AuthorizedPluginTarget,
    ) -> Result<reqwest::Client, HttpError> {
        self.plugin_context.build_pinned_client(target)
    }

    #[cfg(test)]
    pub(super) async fn authorize_plugin_target_for_test(
        &self,
        plugin_id: &str,
        url: url::Url,
    ) -> Result<AuthorizedPluginTarget, HttpError> {
        self.plugin_context.authorize_target(plugin_id, url).await
    }

    /// Update the whitelist
    pub fn update_whitelist(&self, whitelist: DomainWhitelist) {
        *self.plugin_context.whitelist.write() = whitelist;
    }

    /// Approve a domain for a plugin
    pub fn approve_domain(&self, plugin_id: &str, domain: &str) {
        self.plugin_context
            .whitelist
            .write()
            .approve(plugin_id, domain);
        info!(
            "Auto-approved domain '{}' for plugin '{}'",
            domain, plugin_id
        );
    }

    /// Replace the domains granted by this plugin's currently loaded
    /// manifest. Independent user approvals remain intact.
    pub fn replace_plugin_manifest_domains(&self, plugin_id: &str, domains: &[String]) {
        self.plugin_context
            .whitelist
            .read()
            .replace_manifest_domains(plugin_id, domains);
    }

    /// Remove the request policy and manifest-owned domains for an unloaded
    /// plugin while preserving independently approved domains and proxy
    /// preferences.
    pub fn remove_plugin_configuration(&self, plugin_id: &str) {
        self.plugin_context
            .plugin_policies
            .write()
            .remove(plugin_id);
        self.plugin_context
            .whitelist
            .read()
            .clear_manifest_domains(plugin_id);
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
        self.plugin_context.registered_policy(plugin_id)?;
        validate_plugin_url(&request.url)?;
        validate_plugin_headers(&request.headers)?;
        Ok(self.start_plugin_request(plugin_id.to_string(), request))
    }

    fn start_plugin_request(&self, plugin_id: String, request: HttpRequest) -> RequestId {
        let id = RequestId::new();
        let completion = RequestCompletion::new(RequestStatus::Pending);
        self.pending.lock().insert(
            id.clone(),
            PendingEntry {
                completion: completion.clone(),
            },
        );
        let context = self.plugin_context.clone();
        let request_id = id.clone();

        self.runtime.spawn(async move {
            completion.set(RequestStatus::InProgress);
            let status = match context.execute(&plugin_id, request).await {
                Ok(response) => response_to_status(response, &request_id).await,
                Err(error) => RequestStatus::Failed(error.to_string()),
            };
            completion.set(status);
        });

        id
    }

    /// Start a request without plugin restrictions (for host use)
    pub fn request(&self, request: HttpRequest) -> RequestId {
        // Check rate limit (still applies for courtesy)
        if let Ok(domain_info) = analyze_url(&request.url) {
            let rate_limiter = self.plugin_context.rate_limiter.read();
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
            self.plugin_context
                .proxy_runtime
                .read()
                .client_proxied
                .clone()
        } else {
            self.client_direct.clone()
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
            let url = fix_dlsite_cdn_folder(&request.url);

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
        self.plugin_context
            .rate_limiter
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
        self.blocking_get_streaming_with_metadata(
            url,
            use_proxy,
            writer,
            start_byte,
            if_match,
            |_| Ok(()),
        )
    }

    /// Host streaming GET with a validated metadata callback. The callback is
    /// invoked exactly once after response-header validation and before any
    /// body byte is written. A callback error aborts with zero new body bytes.
    pub fn blocking_get_streaming_with_metadata<W, F>(
        &self,
        url: &str,
        use_proxy: bool,
        writer: &mut W,
        start_byte: Option<u64>,
        if_match: Option<&str>,
        on_metadata: F,
    ) -> Result<StreamingDownload, String>
    where
        W: std::io::Write,
        F: FnOnce(&StreamingResponseMetadata) -> Result<(), String>,
    {
        let client = if use_proxy {
            self.plugin_context
                .proxy_runtime
                .read()
                .client_proxied
                .clone()
        } else {
            self.client_direct.clone()
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

            let response = req
                .timeout(crate::DEFAULT_REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;
            write_streaming_response_with_metadata(response, writer, start, on_metadata).await
        })
    }

    /// Checked streaming GET for a plugin. Every redirect uses the same
    /// capability, DNS pinning, whitelist, and rate-limit authorization path.
    pub fn blocking_get_streaming_for_plugin<W: std::io::Write>(
        &self,
        plugin_id: &str,
        url: &str,
        writer: &mut W,
        start_byte: Option<u64>,
        if_match: Option<&str>,
    ) -> Result<StreamingDownload, HttpError> {
        self.blocking_get_streaming_for_plugin_with_metadata(
            plugin_id,
            url,
            writer,
            start_byte,
            if_match,
            |_| Ok(()),
        )
    }

    /// Checked plugin streaming GET with a validated metadata callback.
    /// Redirect, capability, whitelist, DNS, and rate-limit authorization all
    /// remain inside the checked executor.
    pub fn blocking_get_streaming_for_plugin_with_metadata<W, F>(
        &self,
        plugin_id: &str,
        url: &str,
        writer: &mut W,
        start_byte: Option<u64>,
        if_match: Option<&str>,
        on_metadata: F,
    ) -> Result<StreamingDownload, HttpError>
    where
        W: std::io::Write,
        F: FnOnce(&StreamingResponseMetadata) -> Result<(), String>,
    {
        let mut request = HttpRequest::get(url);
        if let Some(start) = start_byte {
            request = request.with_header("Range", format!("bytes={start}-"));
        }
        if let Some(etag) = if_match {
            request = request.with_header("If-Match", etag);
        }

        self.runtime.block_on(async {
            let response = self.plugin_context.execute(plugin_id, request).await?;
            write_streaming_response_with_metadata(response, writer, start_byte, on_metadata)
                .await
                .map_err(|message| HttpError::RequestFailed { message })
        })
    }

    /// Checked buffered GET for a plugin.
    pub fn blocking_get_for_plugin(
        &self,
        plugin_id: &str,
        url: &str,
    ) -> Result<Vec<u8>, HttpError> {
        self.runtime.block_on(async {
            let response = self
                .plugin_context
                .execute(plugin_id, HttpRequest::get(url))
                .await?;
            if !response.status().is_success() {
                return Err(HttpError::RequestFailed {
                    message: format!("HTTP error: {}", response.status()),
                });
            }
            response
                .bytes()
                .await
                .map(|body| body.to_vec())
                .map_err(|error| HttpError::RequestFailed {
                    message: format!("Failed to read body: {error}"),
                })
        })
    }

    /// Blocking GET request (for use from background threads, NOT main thread)
    /// This uses block_on to wait for the async request to complete.
    pub fn blocking_get(&self, url: &str, use_proxy: bool) -> Result<Vec<u8>, String> {
        let client = if use_proxy {
            self.plugin_context
                .proxy_runtime
                .read()
                .client_proxied
                .clone()
        } else {
            self.client_direct.clone()
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

async fn write_streaming_response_with_metadata<W, F>(
    mut response: reqwest::Response,
    writer: &mut W,
    requested_start: Option<u64>,
    on_metadata: F,
) -> Result<StreamingDownload, String>
where
    W: std::io::Write,
    F: FnOnce(&StreamingResponseMetadata) -> Result<(), String>,
{
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP error: {status}"));
    }
    let was_partial = match status.as_u16() {
        200 => false,
        206 => true,
        _ => return Err(format!("Unsupported streaming response status: {status}")),
    };
    let content_range = if was_partial {
        let requested_start = requested_start.ok_or_else(|| {
            "Server returned 206 Partial Content without a Range request".to_string()
        })?;
        let header = response
            .headers()
            .get("content-range")
            .ok_or_else(|| "206 response is missing Content-Range".to_string())?
            .to_str()
            .map_err(|_| "206 response has an invalid Content-Range header".to_string())?;
        let range = parse_content_range(header)
            .ok_or_else(|| format!("206 response has an invalid Content-Range: {header}"))?;
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
                .map_err(|_| "206 response has an invalid Content-Length header".to_string())?
                .parse::<u64>()
                .map_err(|_| "206 response has an invalid Content-Length header".to_string())?;
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
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let total_size = if let Some(range) = content_range {
        Some(range.total)
    } else {
        response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
    };
    let declared_length = content_range.map(|range| range.end - range.start + 1);
    let metadata = StreamingResponseMetadata {
        validated_url: response.url().to_string(),
        was_partial,
        etag: etag.clone(),
        last_modified: last_modified.clone(),
        range_start: content_range.map(|range| range.start),
        total_size,
        expected_body_length: declared_length.or(total_size),
    };
    on_metadata(&metadata)?;
    let mut total_written = 0_u64;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Failed to read body chunk: {error}"))?
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
                        .map_err(|error| format!("Writer failed: {error}"))?;
                }
                return Err(format!(
                    "206 response body exceeds Content-Range: chunk has {chunk_length} bytes with only {remaining} remaining"
                ));
            }
        }

        writer
            .write_all(&chunk)
            .map_err(|error| format!("Writer failed: {error}"))?;
        total_written = next_total;
    }

    writer
        .flush()
        .map_err(|error| format!("Writer flush failed: {error}"))?;
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
}

async fn response_to_status(response: reqwest::Response, request_id: &RequestId) -> RequestStatus {
    let status_code = response.status().as_u16();
    let headers: HashMap<String, String> = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
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
        Err(error) => {
            warn!("Request {} body error: {}", request_id.0, error);
            RequestStatus::Failed(format!("Failed to read body: {error}"))
        }
    }
}

/// Fix DLSite CDN URLs with incorrectly-padded folder names.
/// Folder digit count should match the product ID digit count.
/// e.g., /RJ00361000/RJ360420_ -> /RJ361000/RJ360420_ (6 digits like product)
///       /VJ01006000/VJ01005126_ -> unchanged (8 digits matches product)
fn fix_dlsite_cdn_folder(url: &str) -> String {
    let is_image_host = url::Url::parse(url)
        .ok()
        .is_some_and(|parsed| parsed.host_str() == Some("img.dlsite.jp"));
    if !is_image_host {
        return url.to_string();
    }

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
                                debug!("Fixed DLSite CDN folder: {} -> {}", old_folder, new_folder);
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
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed.host_str().map(|host| {
                ["dlsite.com", "dlsite.jp"].iter().any(|apex| {
                    host == *apex
                        || host
                            .strip_suffix(apex)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                })
            })
        })
        .unwrap_or(false)
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
    use super::{fix_dlsite_cdn_folder, is_dlsite_url, AsyncHttpClient};
    use crate::features::request::PluginNetworkPolicy;
    use crate::features::whitelist::DomainWhitelist;
    use crate::HttpRequest;
    use parking_lot::RwLock;
    use std::sync::Arc;
    use tokio::runtime::Handle;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn matches_dlsite_com() {
        assert!(is_dlsite_url(
            "https://www.dlsite.com/maniax/work/=/product_id/RJ123.html"
        ));
    }

    #[test]
    fn matches_dlsite_jp() {
        assert!(is_dlsite_url(
            "https://img.dlsite.jp/modpub/images2/work/.../img.jpg"
        ));
    }

    #[test]
    fn matches_dlsite_apexes_and_label_boundary_subdomains() {
        for url in [
            "https://dlsite.com/",
            "https://DLsite.JP/",
            "https://api.www.dlsite.com/file",
            "https://img.dlsite.jp/file",
        ] {
            assert!(is_dlsite_url(url), "DLsite host did not match: {url}");
        }
    }

    #[test]
    fn does_not_match_dlsite_text_outside_the_canonical_host() {
        for url in [
            "https://example.com/file",
            "https://dlsite.example.org/spoof",
            "https://evil-dlsite.com/file",
            "https://dlsite.com.attacker.example/file",
            "https://example.com/dlsite.com/file",
            "https://example.com/file?origin=img.dlsite.jp",
            "not a URL containing dlsite.jp",
        ] {
            assert!(!is_dlsite_url(url), "unrelated URL matched DLsite: {url}");
        }
    }

    #[test]
    fn cdn_folder_fix_only_applies_to_the_exact_image_host() {
        let path = "/modpub/images2/work/doujin/RJ00361000/RJ361000_img_main.jpg";
        assert_eq!(
            fix_dlsite_cdn_folder(&format!("https://img.dlsite.jp{path}")),
            format!(
                "https://img.dlsite.jp{}",
                path.replace("RJ00361000", "RJ361000")
            )
        );

        for url in [
            format!("https://cdn.img.dlsite.jp{path}"),
            format!("https://img.dlsite.jp.attacker.example{path}"),
            format!("https://example.com/img.dlsite.jp{path}"),
            format!("https://example.com{path}?origin=img.dlsite.jp"),
        ] {
            assert_eq!(
                fix_dlsite_cdn_folder(&url),
                url,
                "unrelated authority received the DLsite CDN rewrite"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn checked_plugin_redirect_does_not_inject_dlsite_secrets_on_unrelated_host() {
        let server = MockServer::start().await;
        let final_path = "/assets/dlsite.com/final";
        let final_url = format!(
            "http://final.test:{}{final_path}?origin=img.dlsite.jp",
            server.address().port()
        );
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(ResponseTemplate::new(302).append_header("Location", final_url.as_str()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(final_path))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let whitelist = Arc::new(RwLock::new(DomainWhitelist::default()));
        whitelist.write().approve("plugin-a", "start.test");
        whitelist.write().approve("plugin-a", "final.test");
        let client = AsyncHttpClient::new(Handle::current(), whitelist, None);
        client.configure_plugin(
            "plugin-a",
            PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 60,
            },
        );
        client.allow_special_plugin_addresses_for_test();
        client.set_plugin_dns_answers_for_test("start.test", vec![vec![*server.address()]]);
        client.set_plugin_dns_answers_for_test("final.test", vec![vec![*server.address()]]);

        let start_url = format!("http://start.test:{}/start", server.address().port());
        let response = client
            .plugin_context
            .execute("plugin-a", HttpRequest::get(start_url))
            .await
            .expect("checked redirect request should succeed");
        assert_eq!(response.status().as_u16(), 200);

        let requests = server
            .received_requests()
            .await
            .expect("record checked redirect requests");
        let redirected = requests
            .iter()
            .find(|request| request.url.path() == final_path)
            .expect("redirect target request was not received");
        assert!(!redirected.headers.contains_key("cookie"));
        assert!(!redirected.headers.contains_key("referer"));
    }
}

#[cfg(test)]
mod proxy_routing_atomicity_tests {
    use super::{AsyncHttpClient, ProxyRuntimeState};
    use crate::features::proxy::ProxyConfig;
    use crate::features::request::PluginNetworkPolicy;
    use crate::features::whitelist::DomainWhitelist;
    use crate::{HttpRequest, RequestStatus};
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::runtime::Handle;

    async fn observe_one_socks_connect(listener: TcpListener) -> std::net::SocketAddr {
        let (mut socket, _) = listener.accept().await.expect("accept SOCKS5 client");
        let mut greeting = [0_u8; 3];
        socket
            .read_exact(&mut greeting)
            .await
            .expect("read SOCKS5 greeting");
        assert_eq!(greeting, [0x05, 0x01, 0x00]);
        socket
            .write_all(&[0x05, 0x00])
            .await
            .expect("select SOCKS5 no-auth");

        let mut prefix = [0_u8; 4];
        socket
            .read_exact(&mut prefix)
            .await
            .expect("read SOCKS5 CONNECT prefix");
        assert_eq!(&prefix[..3], &[0x05, 0x01, 0x00]);
        assert_eq!(prefix[3], 0x01, "checked routing must use pinned IPv4");
        let mut address = [0_u8; 4];
        socket
            .read_exact(&mut address)
            .await
            .expect("read SOCKS5 destination address");
        let mut port = [0_u8; 2];
        socket
            .read_exact(&mut port)
            .await
            .expect("read SOCKS5 destination port");
        socket
            .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .expect("acknowledge SOCKS5 CONNECT");

        std::net::SocketAddr::new(address.into(), u16::from_be_bytes(port))
    }

    async fn serve_one_host_request(listener: TcpListener) {
        let (mut socket, _) = listener.accept().await.expect("accept host request");
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await.expect("read host request");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .expect("write host response");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unavailable_plugin_routing_denies_plugins_but_keeps_host_requests_available() {
        const PLUGIN_ID: &str = "unavailable-routing-plugin";

        let host_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind host sentinel");
        let host_address = host_listener.local_addr().expect("host sentinel address");
        let host_server = tokio::spawn(serve_one_host_request(host_listener));

        let client = AsyncHttpClient::new(
            Handle::current(),
            Arc::new(RwLock::new(DomainWhitelist::default())),
            None,
        );
        client.configure_plugin(
            PLUGIN_ID,
            PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 60,
            },
        );

        client.mark_plugin_routing_unavailable();
        let denied = client
            .plugin_context
            .registered_policy(PLUGIN_ID)
            .expect_err("checked plugin routing must fail closed while unavailable");
        assert!(
            denied.to_string().contains("routing is unavailable"),
            "unexpected unavailable-routing error: {denied}"
        );

        let host_request = client.request(HttpRequest::get(format!("http://{host_address}/")));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), client.await_complete(&host_request))
                .await
                .expect("host request timed out"),
            Some(RequestStatus::Ready(_))
        ));
        host_server.await.expect("host sentinel panicked");

        client.apply_proxy_routing(None, HashMap::new());
        client
            .plugin_context
            .registered_policy(PLUGIN_ID)
            .expect("a valid routing apply must clear the unavailable state");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unavailable_routing_activation_is_atomic_with_checked_requests() {
        const PLUGIN_ID: &str = "unavailable-routing-race-plugin";
        const TARGET_HOST: &str = "unavailable-routing-race.test";

        let direct_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind direct sentinel");
        let direct_address = direct_listener
            .local_addr()
            .expect("direct sentinel address");
        let proxy_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind SOCKS5 sentinel");
        let proxy_address = proxy_listener.local_addr().expect("SOCKS5 address");

        let whitelist = Arc::new(RwLock::new(DomainWhitelist::default()));
        whitelist.write().approve(PLUGIN_ID, TARGET_HOST);
        let client = Arc::new(AsyncHttpClient::new(Handle::current(), whitelist, None));
        client.configure_plugin(
            PLUGIN_ID,
            PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 60,
            },
        );
        client.allow_special_plugin_addresses_for_test();
        client.set_plugin_dns_answers_for_test(TARGET_HOST, vec![vec![direct_address]]);
        client.apply_proxy_routing(
            Some(ProxyConfig {
                enabled: true,
                address: proxy_address.to_string(),
                username: None,
                password: None,
            }),
            HashMap::from([(PLUGIN_ID.to_string(), true)]),
        );

        let (installed_sender, installed_receiver) = mpsc::sync_channel(0);
        let (release_sender, release_receiver) = mpsc::sync_channel(0);
        let transition_client = client.clone();
        let transition = std::thread::spawn(move || {
            let replacement = ProxyRuntimeState::unavailable();
            let mut runtime = transition_client.plugin_context.proxy_runtime.write();
            *runtime = replacement;
            installed_sender
                .send(())
                .expect("announce unavailable routing state");
            release_receiver
                .recv()
                .expect("receive routing-state release");
        });
        installed_receiver
            .recv()
            .expect("routing transition did not install unavailable state");

        let (request_sender, request_receiver) = mpsc::channel();
        let request_client = client.clone();
        let request_thread = std::thread::spawn(move || {
            request_sender
                .send(
                    request_client.request_for_plugin(
                        PLUGIN_ID,
                        HttpRequest::get(format!(
                            "http://{TARGET_HOST}:{}/resource",
                            direct_address.port()
                        ))
                        .with_timeout(Duration::from_secs(1)),
                    ),
                )
                .expect("report synchronous request preflight");
        });
        let request = match request_receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(result) => result.expect("checked request should start while routing is locked"),
            Err(error) => {
                release_sender
                    .send(())
                    .expect("release routing transition after blocked preflight");
                transition.join().expect("routing transition panicked");
                request_thread.join().expect("request preflight panicked");
                panic!("synchronous request preflight blocked on routing: {error}");
            }
        };
        request_thread.join().expect("request preflight panicked");

        tokio::time::timeout(Duration::from_secs(1), async {
            while client.plugin_dns_lookup_count_for_test(TARGET_HOST) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("checked request did not reach post-DNS authorization");

        release_sender.send(()).expect("release routing transition");
        transition.join().expect("routing transition panicked");

        let (direct_connection, proxy_connection) = tokio::join!(
            tokio::time::timeout(Duration::from_millis(250), direct_listener.accept()),
            tokio::time::timeout(Duration::from_millis(250), proxy_listener.accept()),
        );
        assert!(
            direct_connection.is_err(),
            "checked request reached the direct sentinel after routing became unavailable"
        );
        assert!(
            proxy_connection.is_err(),
            "checked request reached the proxy sentinel after routing became unavailable"
        );
        assert!(
            matches!(
                client.await_complete(&request).await,
                Some(RequestStatus::Failed(message))
                    if message.contains("routing is unavailable")
            ),
            "checked request must fail with the unavailable-routing error"
        );
    }

    async fn assert_overlapping_enable_never_reaches_direct(
        initial_proxy: Option<ProxyConfig>,
        initial_map: HashMap<String, bool>,
    ) {
        const PLUGIN_ID: &str = "routing-race-plugin";
        const TARGET_HOST: &str = "routing-race.test";

        let direct_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind direct sentinel");
        let direct_address = direct_listener
            .local_addr()
            .expect("direct sentinel address");
        let proxy_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind SOCKS5 sentinel");
        let proxy_address = proxy_listener.local_addr().expect("SOCKS5 address");
        let proxy_observer = tokio::spawn(observe_one_socks_connect(proxy_listener));

        let whitelist = Arc::new(RwLock::new(DomainWhitelist::default()));
        whitelist.write().approve(PLUGIN_ID, TARGET_HOST);
        let client = Arc::new(AsyncHttpClient::new(
            Handle::current(),
            whitelist,
            initial_proxy.clone(),
        ));
        client.configure_plugin(
            PLUGIN_ID,
            PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 60,
            },
        );
        client.allow_special_plugin_addresses_for_test();
        client.set_plugin_dns_answers_for_test(TARGET_HOST, vec![vec![direct_address]]);
        client.apply_proxy_routing(initial_proxy, initial_map);

        let enabled_proxy = ProxyConfig {
            enabled: true,
            address: proxy_address.to_string(),
            username: None,
            password: None,
        };
        let enabled_map = HashMap::from([(PLUGIN_ID.to_string(), true)]);
        let (installed_sender, installed_receiver) = mpsc::sync_channel(0);
        let (release_sender, release_receiver) = mpsc::sync_channel(0);
        let transition_client = client.clone();
        let transition = std::thread::spawn(move || {
            let replacement = ProxyRuntimeState::new(Some(enabled_proxy), enabled_map);
            let mut runtime = transition_client.plugin_context.proxy_runtime.write();
            *runtime = replacement;
            installed_sender
                .send(())
                .expect("announce installed routing state");
            release_receiver
                .recv()
                .expect("receive routing-state release");
        });
        installed_receiver
            .recv()
            .expect("routing transition did not install state");

        let request = client
            .request_for_plugin(
                PLUGIN_ID,
                HttpRequest::get(format!(
                    "http://{TARGET_HOST}:{}/resource",
                    direct_address.port()
                ))
                .with_timeout(Duration::from_secs(1)),
            )
            .expect("checked request should start");
        tokio::time::timeout(Duration::from_secs(1), async {
            while client.plugin_dns_lookup_count_for_test(TARGET_HOST) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("checked request did not reach authorization");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), direct_listener.accept())
                .await
                .is_err(),
            "checked request escaped directly while routing activation was locked"
        );

        release_sender.send(()).expect("release routing transition");
        transition.join().expect("routing transition panicked");
        let destination = tokio::time::timeout(Duration::from_secs(2), proxy_observer)
            .await
            .expect("checked request never reached SOCKS5")
            .expect("SOCKS5 observer panicked");
        assert_eq!(destination, direct_address);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), direct_listener.accept())
                .await
                .is_err(),
            "checked request reached the direct target after proxy activation"
        );
        assert!(
            matches!(
                client.await_complete(&request).await,
                Some(RequestStatus::Failed(_))
            ),
            "sentinel SOCKS5 connection should close before an HTTP response"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn global_proxy_enable_is_atomic_with_checked_request_routing() {
        assert_overlapping_enable_never_reaches_direct(None, HashMap::new()).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn per_plugin_proxy_opt_in_is_atomic_with_checked_request_routing() {
        assert_overlapping_enable_never_reaches_direct(
            Some(ProxyConfig {
                enabled: true,
                address: "127.0.0.1:9".to_string(),
                username: None,
                password: None,
            }),
            HashMap::from([("routing-race-plugin".to_string(), false)]),
        )
        .await;
    }
}
