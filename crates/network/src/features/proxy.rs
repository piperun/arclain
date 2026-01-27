use std::error::Error;

/// Result of a single test step
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionTestStep {
    pub name: String,
    pub passed: bool,
    pub message: Option<String>,
}

/// Complete connection test result with all steps
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionTestResult {
    pub steps: Vec<ConnectionTestStep>,
    pub success: bool,
    pub ip: Option<String>,
    pub country: Option<String>,
}

/// Proxy configuration
#[derive(Clone, Debug, Default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub address: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    pub fn to_proxy(&self) -> Option<reqwest::Proxy> {
        if !self.enabled || self.address.is_empty() {
            tracing::debug!("[ProxyConfig] to_proxy: disabled or empty address");
            return None;
        }

        // Use socks5h:// (with 'h') to perform DNS resolution through the proxy
        // This is required for hostnames that are only resolvable via the proxy
        let url = match (&self.username, &self.password) {
            (Some(u), Some(p)) => format!("socks5h://{}:{}@{}", u, p, self.address),
            _ => format!("socks5h://{}", self.address),
        };

        tracing::info!("[ProxyConfig] Creating proxy with URL: {}", url);
        let result = reqwest::Proxy::all(&url);
        match &result {
            Ok(_) => tracing::info!("[ProxyConfig] Proxy created successfully"),
            Err(e) => tracing::error!("[ProxyConfig] Failed to create proxy: {}", e),
        }
        result.ok()
    }

    /// Test the connection with current configuration
    /// Returns a structured result with test steps
    pub async fn test_connection(&self) -> ConnectionTestResult {
        let mut result = ConnectionTestResult::default();
        let test_name = if self.enabled { "SOCKS5" } else { "HTTP" };

        tracing::info!(
            "[test_connection] Starting test: enabled={}, address='{}'",
            self.enabled,
            self.address
        );

        // Try to resolve the proxy hostname to see if DNS works
        if self.enabled && !self.address.is_empty() {
            tracing::info!("[test_connection] Resolving proxy hostname...");
            match tokio::net::lookup_host(&self.address).await {
                Ok(addrs) => {
                    let addrs: Vec<_> = addrs.collect();
                    let ip_str = addrs
                        .first()
                        .map(|a| a.ip().to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    tracing::info!("[test_connection] Proxy DNS resolved to: {:?}", addrs);
                    result.steps.push(ConnectionTestStep {
                        name: "DNS".to_string(),
                        passed: true,
                        message: Some(format!("Resolved to {}", ip_str)),
                    });
                }
                Err(e) => {
                    tracing::error!("[test_connection] Failed to resolve proxy hostname: {}", e);
                    result.steps.push(ConnectionTestStep {
                        name: "DNS".to_string(),
                        passed: false,
                        message: Some(e.to_string()),
                    });
                    return result;
                }
            }

            // Try direct TCP connection to the proxy
            tracing::info!("[test_connection] Attempting direct TCP connection to proxy...");
            match tokio::net::TcpStream::connect(&self.address).await {
                Ok(stream) => {
                    tracing::info!(
                        "[test_connection] Direct TCP connection succeeded! Local: {:?}, Peer: {:?}",
                        stream.local_addr(),
                        stream.peer_addr()
                    );
                    result.steps.push(ConnectionTestStep {
                        name: "TCP".to_string(),
                        passed: true,
                        message: None,
                    });
                    drop(stream);
                }
                Err(e) => {
                    tracing::error!(
                        "[test_connection] Direct TCP connection failed: {} (kind: {:?})",
                        e,
                        e.kind()
                    );
                    result.steps.push(ConnectionTestStep {
                        name: "TCP".to_string(),
                        passed: false,
                        message: Some(e.to_string()),
                    });
                    return result;
                }
            }
        }

        let mut builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(10))
            .no_proxy(); // Disable system proxy detection

        if let Some(proxy) = self.to_proxy() {
            tracing::info!("[test_connection] Applying proxy to client builder");
            builder = builder.proxy(proxy);
        } else if self.enabled {
            tracing::warn!("[test_connection] Proxy enabled but to_proxy() returned None");
            result.steps.push(ConnectionTestStep {
                name: test_name.to_string(),
                passed: false,
                message: Some("Invalid proxy configuration".to_string()),
            });
            return result;
        }

        let client = match builder.build() {
            Ok(c) => {
                tracing::info!("[test_connection] Client built successfully");
                c
            }
            Err(e) => {
                tracing::error!("[test_connection] Failed to build client: {}", e);
                result.steps.push(ConnectionTestStep {
                    name: test_name.to_string(),
                    passed: false,
                    message: Some(format!("Failed to build client: {}", e)),
                });
                return result;
            }
        };

        // Test connection using ip-api.com for IP and Location data
        tracing::info!("[test_connection] Sending request to http://ip-api.com/json");
        let response = match client.get("http://ip-api.com/json").send().await {
            Ok(r) => {
                tracing::info!("[test_connection] Request succeeded with status: {}", r.status());
                r
            }
            Err(e) => {
                // Log the full error chain
                tracing::error!("[test_connection] Request failed: {}", e);
                let mut depth = 0;
                let mut source = e.source();
                while let Some(s) = source {
                    depth += 1;
                    tracing::error!("[test_connection] Cause {}: {}", depth, s);
                    source = s.source();
                }

                // Find the root cause (deepest error in the chain)
                let mut root_cause = e.to_string();
                let mut source = e.source();
                while let Some(s) = source {
                    root_cause = s.to_string();
                    source = s.source();
                }
                result.steps.push(ConnectionTestStep {
                    name: test_name.to_string(),
                    passed: false,
                    message: Some(root_cause),
                });
                return result;
            }
        };

        if !response.status().is_success() {
            result.steps.push(ConnectionTestStep {
                name: test_name.to_string(),
                passed: false,
                message: Some(format!("HTTP error: {}", response.status())),
            });
            return result;
        }

        let json: serde_json::Value = match response.json().await {
            Ok(j) => j,
            Err(e) => {
                result.steps.push(ConnectionTestStep {
                    name: test_name.to_string(),
                    passed: false,
                    message: Some(format!("Failed to parse JSON: {}", e)),
                });
                return result;
            }
        };

        let ip = json
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown IP");
        let country = json
            .get("country")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Country");

        // Test passed
        result.steps.push(ConnectionTestStep {
            name: test_name.to_string(),
            passed: true,
            message: None,
        });

        result.success = true;
        result.ip = Some(ip.to_string());
        result.country = Some(country.to_string());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_disabled() {
        let config = ProxyConfig {
            enabled: false,
            address: "127.0.0.1:9050".to_string(),
            ..Default::default()
        };
        assert!(config.to_proxy().is_none());
    }

    #[test]
    fn test_proxy_empty_address() {
        let config = ProxyConfig {
            enabled: true,
            address: "".to_string(),
            ..Default::default()
        };
        assert!(config.to_proxy().is_none());
    }

    #[test]
    fn test_proxy_enabled_no_auth() {
        let config = ProxyConfig {
            enabled: true,
            address: "127.0.0.1:9050".to_string(),
            ..Default::default()
        };
        assert!(config.to_proxy().is_some());
    }

    #[test]
    fn test_proxy_enabled_with_auth() {
        let config = ProxyConfig {
            enabled: true,
            address: "127.0.0.1:9050".to_string(),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
        };
        assert!(config.to_proxy().is_some());
    }
}
