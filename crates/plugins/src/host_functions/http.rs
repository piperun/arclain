//! HTTP client operations

use crate::types::{PluginCapability, PluginError, Result};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;

/// HTTP request rate limiter
#[derive(Debug, Clone)]
pub struct RateLimiter {
    requests_per_minute: u32,
    requests: Arc<Mutex<Vec<Instant>>>,
}

impl RateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            requests_per_minute,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn check_rate_limit(&self) -> bool {
        let now = Instant::now();
        let mut requests = self.requests.lock();
        requests.retain(|&time| now.duration_since(time) < Duration::from_secs(60));

        if requests.len() < self.requests_per_minute as usize {
            requests.push(now);
            true
        } else {
            false
        }
    }
}

/// HTTP client for making requests
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::blocking::Client,
    rate_limiter: RateLimiter,
}

impl HttpClient {
    pub fn new(requests_per_minute: u32) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Archust-Plugin/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            rate_limiter: RateLimiter::new(requests_per_minute),
        }
    }

    pub fn get(&self, url: &str) -> Result<String> {
        if !self.rate_limiter.check_rate_limit() {
            return Err(PluginError::ExecutionError(
                "Rate limit exceeded".to_string(),
            ));
        }
        debug!("HTTP GET: {}", url);
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| PluginError::ExecutionError(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(PluginError::ExecutionError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        response
            .text()
            .map_err(|e| PluginError::ExecutionError(format!("Failed to read response: {}", e)))
    }

    pub fn post_json(&self, url: &str, body: &str) -> Result<String> {
        if !self.rate_limiter.check_rate_limit() {
            return Err(PluginError::ExecutionError(
                "Rate limit exceeded".to_string(),
            ));
        }
        debug!("HTTP POST: {}", url);
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| PluginError::ExecutionError(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(PluginError::ExecutionError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }
        response
            .text()
            .map_err(|e| PluginError::ExecutionError(format!("Failed to read response: {}", e)))
    }
}

/// HTTP implementation for HostFunctions
use super::HostFunctions;
use crate::arclain::plugin::host::Host;

impl HostFunctions {
    pub(super) fn impl_http_get(&mut self, url: String) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::Network) {
            return Err("Network capability not granted".to_string());
        }
        let client = self
            .http_client
            .as_ref()
            .ok_or("HTTP client not initialized")?
            .clone();

        // Log the request
        self.log_network_activity(format!("GET {}", url));

        let result = client.get(&url).map_err(|e| e.to_string());

        // Log result
        match &result {
            Ok(resp) => self.log_network_activity(format!("Response: {} bytes", resp.len())),
            Err(e) => self.log_network_activity(format!("Error: {}", e)),
        }

        result
    }

    pub(super) fn impl_http_post(
        &mut self,
        url: String,
        body: String,
    ) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::Network) {
            return Err("Network capability not granted".to_string());
        }
        let client = self
            .http_client
            .as_ref()
            .ok_or("HTTP client not initialized")?
            .clone();

        // Log the request
        self.log_network_activity(format!("POST {}", url));

        let result = client.post_json(&url, &body).map_err(|e| e.to_string());

        // Log result
        match &result {
            Ok(resp) => self.log_network_activity(format!("Response: {} bytes", resp.len())),
            Err(e) => self.log_network_activity(format!("Error: {}", e)),
        }

        result
    }

    pub(super) fn impl_start_async_fetch(&mut self, url: String) -> String {
        if !self.check_capability(PluginCapability::Network) {
            return "error:no_network_capability".to_string();
        }

        let client = match self.http_client.as_ref() {
            Some(c) => c.clone(),
            None => return "error:client_not_initialized".to_string(),
        };

        // Generate simple ID
        let id = format!(
            "req_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        let id_clone = id.clone();
        let results = self.async_results.clone();
        let logger = self.network_log.clone();

        // Initialize as None (pending)
        results.lock().insert(id.clone(), None);

        // Log start
        self.log_network_activity(format!("ASYNC START [{}] GET {}", id, url));

        // Spawn thread for blocking request
        std::thread::spawn(move || {
            let result = client.get(&url).map_err(|e| e.to_string());

            // Log completion
            {
                let mut log = logger.lock();
                let msg = match &result {
                    Ok(resp) => format!("ASYNC DONE [{}] Response: {} bytes", id_clone, resp.len()),
                    Err(e) => format!("ASYNC ERROR [{}] {}", id_clone, e),
                };
                log.push((std::time::SystemTime::now(), msg));
            }

            // Store result
            results.lock().insert(id_clone, Some(result));
        });

        id
    }

    pub(super) fn impl_poll_async_fetch(
        &mut self,
        id: String,
    ) -> Option<std::result::Result<String, String>> {
        let mut results = self.async_results.lock();
        if let Some(res_opt) = results.get(&id) {
            if let Some(res) = res_opt {
                // Result is ready, remove and return it (one-time consume)
                let result = res.clone();
                // We keep it in map? Or remove?
                // Better to remove to clean up memory.
                results.remove(&id);
                Some(result)
            } else {
                // Pending
                None
            }
        } else {
            // ID not found (or already consumed)
            None
        }
    }
}
