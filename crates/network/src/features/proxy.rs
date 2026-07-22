use std::fmt;

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
#[derive(Clone, Default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub address: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyConfig")
            .field("enabled", &self.enabled)
            .field("address", &self.address)
            .field("username", &self.username.as_ref().map(|_| "[REDACTED]"))
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl ProxyConfig {
    /// Credential-free status text suitable for logs and diagnostics.
    pub fn log_summary(&self) -> String {
        let enabled = if self.enabled { "enabled" } else { "disabled" };
        let authentication = if self.username.is_some() && self.password.is_some() {
            "authenticated"
        } else {
            "unauthenticated"
        };
        let address = if self.address.trim().is_empty() {
            "<empty address>"
        } else {
            &self.address
        };
        format!("{enabled} SOCKS5 proxy at {address} ({authentication})")
    }

    /// Construct the authenticated proxy URL without exposing it to any
    /// diagnostic surface. The returned string must only be passed to
    /// `reqwest`.
    fn proxy_url(&self) -> Result<String, String> {
        let mut url = url::Url::parse(&format!("socks5h://{}", self.address)).map_err(|_| {
            format!(
                "Invalid SOCKS5 address '{}': {}",
                self.address,
                self.log_summary()
            )
        })?;

        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            url.set_username(username)
                .map_err(|_| format!("Invalid credentials for {}", self.log_summary()))?;
            url.set_password(Some(password))
                .map_err(|_| format!("Invalid credentials for {}", self.log_summary()))?;
        }

        Ok(url.into())
    }

    fn create_proxy(&self) -> Result<reqwest::Proxy, String> {
        let proxy_url = self.proxy_url()?;
        reqwest::Proxy::all(&proxy_url)
            .map_err(|_| format!("Failed to create {}", self.log_summary()))
    }

    /// Build a `reqwest::Proxy` from this config. Returns `None` when
    /// the config is disabled OR when the address fails to parse —
    /// callers can't tell these two cases apart, which is the M4
    /// silent-disable bug. Use [`ProxyConfig::validate`] when the
    /// distinction matters (e.g. saving user-entered settings).
    pub fn to_proxy(&self) -> Option<reqwest::Proxy> {
        if !self.enabled || self.address.is_empty() {
            tracing::debug!("[ProxyConfig] Skipping {}", self.log_summary());
            return None;
        }

        tracing::info!("[ProxyConfig] Creating {}", self.log_summary());
        match self.create_proxy() {
            Ok(proxy) => {
                tracing::info!("[ProxyConfig] Created {}", self.log_summary());
                Some(proxy)
            }
            Err(error) => {
                tracing::error!("[ProxyConfig] {}", error);
                None
            }
        }
    }

    /// Validate that the config can be turned into a usable proxy.
    ///
    /// Returns `Ok(())` when the config is disabled (no proxy needed)
    /// OR when the address parses cleanly. Returns a user-readable
    /// `Err(String)` when proxying is enabled but the address fails to
    /// parse. Used by the settings save flow to surface invalid input
    /// instead of silently disabling the proxy (audit finding M4).
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.address.trim().is_empty() {
            return Err(format!("Invalid {}: address is empty", self.log_summary()));
        }
        self.create_proxy().map(|_| ())
    }

    /// Test the connection with current configuration
    /// Returns a structured result with test steps
    pub async fn test_connection(&self) -> ConnectionTestResult {
        let mut result = ConnectionTestResult::default();
        let test_name = if self.enabled { "SOCKS5" } else { "HTTP" };

        tracing::info!(
            "[test_connection] Starting test using {}",
            self.log_summary()
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
            .connect_timeout(crate::PROBE_TIMEOUT)
            .timeout(crate::PROBE_TIMEOUT)
            .no_proxy(); // Disable system proxy detection

        if let Some(proxy) = self.to_proxy() {
            tracing::info!("[test_connection] Applying proxy to client builder");
            builder = builder.proxy(proxy);
        } else if self.enabled {
            let message = format!("Invalid {}", self.log_summary());
            tracing::warn!("[test_connection] {}", message);
            result.steps.push(ConnectionTestStep {
                name: test_name.to_string(),
                passed: false,
                message: Some(message),
            });
            return result;
        }

        let client = match builder.build() {
            Ok(c) => {
                tracing::info!("[test_connection] Client built successfully");
                c
            }
            Err(_) => {
                let message = format!("Failed to build client using {}", self.log_summary());
                tracing::error!("[test_connection] {}", message);
                result.steps.push(ConnectionTestStep {
                    name: test_name.to_string(),
                    passed: false,
                    message: Some(message),
                });
                return result;
            }
        };

        // Test connection using ip-api.com for IP and Location data
        tracing::info!("[test_connection] Sending request to http://ip-api.com/json");
        let response = match client.get("http://ip-api.com/json").send().await {
            Ok(r) => {
                tracing::info!(
                    "[test_connection] Request succeeded with status: {}",
                    r.status()
                );
                r
            }
            Err(_) => {
                let message = format!("Request failed using {}", self.log_summary());
                tracing::error!("[test_connection] {}", message);
                result.steps.push(ConnectionTestStep {
                    name: test_name.to_string(),
                    passed: false,
                    message: Some(message),
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
    use std::fmt::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    const USERNAME_SECRET: &str = "proxy-username-secret-7e1f";
    const PASSWORD_SECRET: &str = "proxy-password-secret-9a4c";

    fn authenticated_config(address: &str) -> ProxyConfig {
        ProxyConfig {
            enabled: true,
            address: address.to_string(),
            username: Some(USERNAME_SECRET.to_string()),
            password: Some(PASSWORD_SECRET.to_string()),
        }
    }

    fn assert_credentials_redacted(text: &str) {
        assert!(
            !text.contains(USERNAME_SECRET),
            "username leaked in {text:?}"
        );
        assert!(
            !text.contains(PASSWORD_SECRET),
            "password leaked in {text:?}"
        );
    }

    #[derive(Clone, Default)]
    struct EventCapture {
        events: Arc<Mutex<Vec<String>>>,
        next_span: Arc<AtomicU64>,
    }

    impl EventCapture {
        fn output(&self) -> String {
            self.events
                .lock()
                .expect("capture lock poisoned")
                .join("\n")
        }
    }

    impl Subscriber for EventCapture {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
            Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            struct EventVisitor(String);

            impl Visit for EventVisitor {
                fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
                    write!(&mut self.0, "{}={value:?} ", field.name())
                        .expect("write captured event");
                }
            }

            let mut visitor = EventVisitor(String::new());
            event.record(&mut visitor);
            self.events
                .lock()
                .expect("capture lock poisoned")
                .push(visitor.0);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    fn capture_events(operation: impl FnOnce()) -> String {
        let capture = EventCapture::default();
        tracing::subscriber::with_default(capture.clone(), operation);
        capture.output()
    }

    #[test]
    fn proxy_debug_redacts_present_credentials() {
        let debug = format!("{:?}", authenticated_config("proxy.example:1080"));

        assert_credentials_redacted(&debug);
        assert_eq!(debug.matches("[REDACTED]").count(), 2);
        assert!(debug.contains("proxy.example:1080"));
    }

    #[test]
    fn proxy_log_summaries_include_state_without_credentials() {
        let authenticated = authenticated_config("proxy.example:1080");
        let authenticated_summary = authenticated.log_summary();
        assert_eq!(
            authenticated_summary,
            "enabled SOCKS5 proxy at proxy.example:1080 (authenticated)"
        );
        assert_credentials_redacted(&authenticated_summary);

        let disabled = ProxyConfig {
            enabled: false,
            address: "proxy.example:1080".to_string(),
            username: Some(USERNAME_SECRET.to_string()),
            password: None,
        };
        let disabled_summary = disabled.log_summary();
        assert_eq!(
            disabled_summary,
            "disabled SOCKS5 proxy at proxy.example:1080 (unauthenticated)"
        );
        assert_credentials_redacted(&disabled_summary);
    }

    #[test]
    fn proxy_creation_success_tracing_redacts_credentials() {
        let config = authenticated_config("proxy.example:1080");
        let events = capture_events(|| {
            assert!(config.to_proxy().is_some());
        });

        assert_credentials_redacted(&events);
        assert!(events.contains("proxy.example:1080"));
        assert!(events.contains("authenticated"));
    }

    #[test]
    fn proxy_creation_error_tracing_redacts_credentials() {
        let config = authenticated_config("not a valid host:1080");
        let events = capture_events(|| {
            assert!(config.to_proxy().is_none());
        });

        assert_credentials_redacted(&events);
        assert!(events.contains("not a valid host:1080"));
        assert!(events.contains("authenticated"));
    }

    #[test]
    fn proxy_validation_errors_redact_credentials() {
        let config = authenticated_config("not a valid host:1080");
        let error = config
            .validate()
            .expect_err("invalid proxy address should be rejected");

        assert_credentials_redacted(&error);
        assert!(error.contains("not a valid host:1080"));
        assert!(error.contains("authenticated"));
    }

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

    /// Regression test for M4 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// Pre-fix, an invalid proxy address silently dropped to `None`
    /// from `to_proxy()`, indistinguishable from "proxy disabled". The
    /// settings-save path would then build an HTTP client with no
    /// proxy attached even though the user thought SOCKS5 was on.
    ///
    /// Post-fix, `validate()` surfaces invalid addresses as `Err` so
    /// the settings controller can show a toast and refuse the save.
    /// This test asserts the new contract directly.
    #[test]
    fn m4_validate_surfaces_invalid_address() {
        let config = ProxyConfig {
            enabled: true,
            // Spaces are not valid in URL authority; reqwest's URL
            // parser rejects this.
            address: "not a valid host:1080".to_string(),
            ..Default::default()
        };

        let result = config.validate();
        assert!(
            result.is_err(),
            "M4 fix regressed: validate() accepted an unparseable address",
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Invalid SOCKS5 address"),
            "Error message should identify the invalid address: {}",
            msg
        );
    }

    /// `validate()` is a no-op when proxy is disabled — only the
    /// "enabled but invalid" combination should surface as `Err`.
    #[test]
    fn m4_validate_passes_when_disabled() {
        let config = ProxyConfig {
            enabled: false,
            address: "not a valid host:1080".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn m4_validate_passes_for_well_formed_address() {
        let config = ProxyConfig {
            enabled: true,
            address: "127.0.0.1:9050".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    /// Documents the pre-fix bug shape for posterity: `to_proxy()`
    /// returns `None` for both "disabled" and "invalid" cases. Callers
    /// that only check for `None` can't tell them apart.
    #[test]
    fn m4_to_proxy_remains_silent_on_invalid_address() {
        let invalid = ProxyConfig {
            enabled: true,
            address: "not a valid host:1080".to_string(),
            ..Default::default()
        };
        let disabled = ProxyConfig {
            enabled: false,
            address: "127.0.0.1:9050".to_string(),
            ..Default::default()
        };
        assert!(invalid.to_proxy().is_none());
        assert!(disabled.to_proxy().is_none());
        // Both look the same to `to_proxy()` — the silent-disable bug.
        // Callers that need to distinguish must use `validate()`.
    }
}
