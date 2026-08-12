use std::fmt;

use crate::shared::safe_log_fingerprint;

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

const INVALID_PROXY_AUTHORITY: &str = "address must contain only a host and a non-zero port";

struct ParsedProxyAuthority {
    url: url::Url,
    authority: String,
}

#[derive(Clone, Copy)]
enum ProxyDnsResolution {
    Remote,
    Local,
}

fn parse_proxy_authority(address: &str) -> Result<ParsedProxyAuthority, &'static str> {
    if address.is_empty()
        || address.trim() != address
        || address
            .chars()
            .any(|character| matches!(character, '@' | '/' | '\\' | '?' | '#'))
    {
        return Err(INVALID_PROXY_AUTHORITY);
    }

    let url =
        url::Url::parse(&format!("socks5h://{address}")).map_err(|_| INVALID_PROXY_AUTHORITY)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(INVALID_PROXY_AUTHORITY);
    }

    let host = match url.host().ok_or(INVALID_PROXY_AUTHORITY)? {
        url::Host::Domain(domain) => domain.to_string(),
        url::Host::Ipv4(address) => address.to_string(),
        url::Host::Ipv6(address) => format!("[{address}]"),
    };
    let port = url
        .port()
        .filter(|port| *port != 0)
        .ok_or(INVALID_PROXY_AUTHORITY)?;

    Ok(ParsedProxyAuthority {
        url,
        authority: format!("{host}:{port}"),
    })
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
        let address = self.diagnostic_address();
        formatter
            .debug_struct("ProxyConfig")
            .field("enabled", &self.enabled)
            .field("address", &address)
            .field("username", &self.username.as_ref().map(|_| "[REDACTED]"))
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl ProxyConfig {
    fn parsed_authority(&self) -> Result<ParsedProxyAuthority, &'static str> {
        parse_proxy_authority(&self.address)
    }

    fn diagnostic_address(&self) -> String {
        self.parsed_authority()
            .map(|parsed| parsed.authority)
            .unwrap_or_else(|_| "<invalid address>".to_string())
    }

    /// Credential-free status text suitable for logs and diagnostics.
    pub fn log_summary(&self) -> String {
        let enabled = if self.enabled { "enabled" } else { "disabled" };
        let authentication = if self.username.is_some() && self.password.is_some() {
            "authenticated"
        } else {
            "unauthenticated"
        };
        let address = self.diagnostic_address();
        format!("{enabled} SOCKS5 proxy at {address} ({authentication})")
    }

    /// Construct the authenticated proxy URL without exposing it to any
    /// diagnostic surface. The returned string must only be passed to
    /// `reqwest`.
    fn proxy_url(&self, dns_resolution: ProxyDnsResolution) -> Result<String, String> {
        let mut url = self
            .parsed_authority()
            .map_err(|reason| {
                format!(
                    "Invalid SOCKS5 address for {}: {reason}",
                    self.log_summary()
                )
            })?
            .url;

        if matches!(dns_resolution, ProxyDnsResolution::Local) {
            url.set_scheme("socks5")
                .map_err(|_| format!("Invalid proxy scheme for {}", self.log_summary()))?;
        }

        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            url.set_username(username)
                .map_err(|_| format!("Invalid credentials for {}", self.log_summary()))?;
            url.set_password(Some(password))
                .map_err(|_| format!("Invalid credentials for {}", self.log_summary()))?;
        }

        Ok(url.into())
    }

    fn create_proxy_with_resolution(
        &self,
        dns_resolution: ProxyDnsResolution,
    ) -> Result<reqwest::Proxy, String> {
        let proxy_url = self.proxy_url(dns_resolution)?;
        reqwest::Proxy::all(&proxy_url)
            .map_err(|_| format!("Failed to create {}", self.log_summary()))
    }

    fn create_proxy(&self) -> Result<reqwest::Proxy, String> {
        self.create_proxy_with_resolution(ProxyDnsResolution::Remote)
    }

    /// Build the locally resolving SOCKS proxy used after plugin DNS has been
    /// validated and pinned. Credential-bearing URL material never leaves this
    /// type, and invalid authorities return only redacted diagnostics.
    pub(crate) fn create_pinned_proxy(&self) -> Result<reqwest::Proxy, String> {
        self.create_proxy_with_resolution(ProxyDnsResolution::Local)
    }

    /// Build a `reqwest::Proxy` from this config. Returns `None` when
    /// the config is disabled OR when the address fails to parse —
    /// callers can't tell these two cases apart, which is the M4
    /// silent-disable bug. Use [`ProxyConfig::validate`] when the
    /// distinction matters (e.g. saving user-entered settings).
    pub fn to_proxy(&self) -> Option<reqwest::Proxy> {
        if !self.enabled || self.address.trim().is_empty() {
            tracing::debug!(
                "[ProxyConfig] Skipping {}",
                safe_log_fingerprint(self.log_summary())
            );
            return None;
        }

        tracing::info!(
            "[ProxyConfig] Creating {}",
            safe_log_fingerprint(self.log_summary())
        );
        match self.create_proxy() {
            Ok(proxy) => {
                tracing::info!(
                    "[ProxyConfig] Created {}",
                    safe_log_fingerprint(self.log_summary())
                );
                Some(proxy)
            }
            Err(error) => {
                tracing::error!("[ProxyConfig] {}", safe_log_fingerprint(error));
                None
            }
        }
    }

    /// Validate that the active transport can be turned into a usable proxy.
    ///
    /// Disabled configurations always pass because no transport will consume
    /// their stored address. Storage boundaries that must reject malformed
    /// nonempty addresses even while disabled should use
    /// [`ProxyConfig::validate_for_storage`].
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        self.create_proxy().map(|_| ())
    }

    /// Validate a proxy configuration before storing it.
    ///
    /// A disabled proxy may have no address, but every nonempty address must
    /// be a strict host-and-port authority regardless of enablement. This
    /// prevents invalid or credential-bearing authorities from crossing a
    /// persistence boundary and being activated later.
    pub fn validate_for_storage(&self) -> Result<(), String> {
        if !self.enabled && self.address.is_empty() {
            return Ok(());
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
            safe_log_fingerprint(self.log_summary())
        );

        // Try to resolve the proxy hostname to see if DNS works
        if self.enabled {
            let authority = match self.parsed_authority() {
                Ok(parsed) => parsed.authority,
                Err(reason) => {
                    let message = format!("Invalid {}: {reason}", self.log_summary());
                    tracing::warn!("[test_connection] {}", safe_log_fingerprint(&message));
                    result.steps.push(ConnectionTestStep {
                        name: test_name.to_string(),
                        passed: false,
                        message: Some(message),
                    });
                    return result;
                }
            };
            tracing::info!("[test_connection] Resolving proxy hostname...");
            match tokio::net::lookup_host(&authority).await {
                Ok(addrs) => {
                    let addrs: Vec<_> = addrs.collect();
                    let ip_str = addrs
                        .first()
                        .map(|a| a.ip().to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    tracing::info!(
                        "[test_connection] Proxy DNS resolved to: {}",
                        safe_log_fingerprint(format!("{addrs:?}"))
                    );
                    result.steps.push(ConnectionTestStep {
                        name: "DNS".to_string(),
                        passed: true,
                        message: Some(format!("Resolved to {}", ip_str)),
                    });
                }
                Err(e) => {
                    tracing::error!(
                        "[test_connection] Failed to resolve proxy hostname: {}",
                        safe_log_fingerprint(e.to_string())
                    );
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
            match tokio::net::TcpStream::connect(&authority).await {
                Ok(stream) => {
                    tracing::info!(
                        "[test_connection] Direct TCP connection succeeded! Local: {}, Peer: {}",
                        safe_log_fingerprint(format!("{:?}", stream.local_addr())),
                        safe_log_fingerprint(format!("{:?}", stream.peer_addr()))
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
                        safe_log_fingerprint(e.to_string()),
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
            tracing::warn!("[test_connection] {}", safe_log_fingerprint(&message));
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
                tracing::error!("[test_connection] {}", safe_log_fingerprint(&message));
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
                tracing::error!("[test_connection] {}", safe_log_fingerprint(&message));
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
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    const USERNAME_SECRET: &str = "proxy-username-secret-7e1f";
    const PASSWORD_SECRET: &str = "proxy-password-secret-9a4c";
    const ADDRESS_USERNAME_SECRET: &str = "address-username-secret-3b8d";
    const ADDRESS_PASSWORD_SECRET: &str = "address-password-secret-5c2a";

    fn authenticated_config(address: &str) -> ProxyConfig {
        ProxyConfig {
            enabled: true,
            address: address.to_string(),
            username: Some(USERNAME_SECRET.to_string()),
            password: Some(PASSWORD_SECRET.to_string()),
        }
    }

    fn embedded_userinfo_config() -> ProxyConfig {
        authenticated_config(&format!(
            "{ADDRESS_USERNAME_SECRET}:{ADDRESS_PASSWORD_SECRET}@proxy.example:1080"
        ))
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
        assert!(
            !text.contains(ADDRESS_USERNAME_SECRET),
            "address username leaked in {text:?}"
        );
        assert!(
            !text.contains(ADDRESS_PASSWORD_SECRET),
            "address password leaked in {text:?}"
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
    fn proxy_creation_success_tracing_fingerprints_the_complete_diagnostic() {
        let config = authenticated_config("proxy.example:1080");
        let events = capture_events(|| {
            assert!(config.to_proxy().is_some());
        });

        assert_credentials_redacted(&events);
        assert!(events.contains("sha256:"));
        assert!(!events.contains("proxy.example:1080"));
        assert!(!events.contains("authenticated"));
    }

    #[test]
    fn proxy_creation_error_tracing_fingerprints_the_complete_diagnostic() {
        let config = authenticated_config("not a valid host:1080");
        let events = capture_events(|| {
            assert!(config.to_proxy().is_none());
        });

        assert_credentials_redacted(&events);
        assert!(events.contains("sha256:"));
        assert!(!events.contains("<invalid address>"));
        assert!(!events.contains("not a valid host:1080"));
        assert!(!events.contains("authenticated"));
    }

    #[test]
    fn proxy_validation_errors_redact_credentials() {
        let config = authenticated_config("not a valid host:1080");
        let error = config
            .validate()
            .expect_err("invalid proxy address should be rejected");

        assert_credentials_redacted(&error);
        assert!(error.contains("<invalid address>"));
        assert!(!error.contains("not a valid host:1080"));
        assert!(error.contains("authenticated"));
    }

    #[test]
    fn proxy_address_userinfo_is_redacted_from_debug() {
        let debug = format!("{:?}", embedded_userinfo_config());

        assert_credentials_redacted(&debug);
        assert!(debug.contains("<invalid address>"));
        assert_eq!(debug.matches("[REDACTED]").count(), 2);
    }

    #[test]
    fn proxy_address_userinfo_is_redacted_from_log_summary() {
        let summary = embedded_userinfo_config().log_summary();

        assert_credentials_redacted(&summary);
        assert_eq!(
            summary,
            "enabled SOCKS5 proxy at <invalid address> (authenticated)"
        );
    }

    #[test]
    fn proxy_address_userinfo_is_rejected_without_echoing_it() {
        let error = embedded_userinfo_config()
            .validate()
            .expect_err("userinfo must not be accepted as a proxy address");

        assert_credentials_redacted(&error);
        assert!(error.contains("<invalid address>"));
        assert!(error.contains("authenticated"));
    }

    #[test]
    fn proxy_address_userinfo_is_fingerprinted_in_captured_tracing() {
        let config = embedded_userinfo_config();
        let capture = EventCapture::default();
        let proxy = tracing::subscriber::with_default(capture.clone(), || config.to_proxy());
        let events = capture.output();

        assert_credentials_redacted(&events);
        assert!(events.contains("sha256:"));
        assert!(!events.contains("<invalid address>"));
        assert!(!events.contains("authenticated"));
        assert!(proxy.is_none());
    }

    #[test]
    fn proxy_validation_requires_a_strict_host_and_port_authority() {
        for address in [
            "user:password@proxy.example:1080",
            "proxy.example:1080/path",
            "proxy.example:1080?query",
            "proxy.example:1080#fragment",
            "socks5h://proxy.example:1080",
            "proxy.example",
            "proxy.example:",
            "proxy.example:0",
        ] {
            let config = ProxyConfig {
                enabled: true,
                address: address.to_string(),
                ..Default::default()
            };
            assert!(
                config.validate().is_err(),
                "non-authority proxy address was accepted: {address:?}"
            );
        }
    }

    #[test]
    fn proxy_validation_preserves_supported_host_and_port_authorities() {
        for address in ["proxy.example:1080", "127.0.0.1:9050", "[2001:db8::1]:1080"] {
            let config = ProxyConfig {
                enabled: true,
                address: address.to_string(),
                ..Default::default()
            };
            assert!(
                config.validate().is_ok(),
                "supported proxy authority was rejected: {address:?}"
            );
        }
    }

    #[test]
    fn proxy_authority_matrix_keeps_ipv4_strict_without_rewriting_it() {
        let cases = [
            ("127.0.0.1:9050", Some("127.0.0.1:9050")),
            (" 127.0.0.1:9050", None),
            ("127.0.0.1:9050 ", None),
            ("127.0.0.1:0", None),
            ("127.0.0.1", None),
            ("127.0.0.1:9050/path", None),
            ("127.0.0.1:9050?query", None),
            ("user:password@127.0.0.1:9050", None),
        ];

        for (address, expected_authority) in cases {
            let parsed = parse_proxy_authority(address).map(|parsed| parsed.authority);
            assert_eq!(
                parsed.as_deref().ok(),
                expected_authority,
                "proxy authority classification changed for {address:?}"
            );
        }
    }

    #[test]
    fn failed_socks_handshake_and_invalid_address_never_expose_credentials() {
        fn accept_before(listener: &TcpListener, deadline: Instant) -> TcpStream {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("make accepted proxy socket blocking");
                        return stream;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "timed out waiting for proxy client"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("proxy listener failed: {error}"),
                }
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local SOCKS listener");
        listener
            .set_nonblocking(true)
            .expect("make SOCKS listener nonblocking");
        let address = listener.local_addr().expect("read SOCKS listener address");
        let server = thread::spawn(move || {
            drop(accept_before(
                &listener,
                Instant::now() + Duration::from_secs(5),
            ));

            let mut socks = accept_before(&listener, Instant::now() + Duration::from_secs(5));
            socks
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set SOCKS read timeout");
            let mut greeting_header = [0_u8; 2];
            socks
                .read_exact(&mut greeting_header)
                .expect("read SOCKS greeting header");
            assert_eq!(greeting_header[0], 0x05, "unexpected SOCKS version");
            let mut methods = vec![0_u8; usize::from(greeting_header[1])];
            socks
                .read_exact(&mut methods)
                .expect("read SOCKS authentication methods");
            socks
                .write_all(&[0x05, 0xff])
                .expect("reject SOCKS authentication methods");
        });

        let valid = authenticated_config(&address.to_string());
        let embedded = embedded_userinfo_config();
        let capture = EventCapture::default();
        let (handshake_result, embedded_result) =
            tracing::subscriber::with_default(capture.clone(), || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build test runtime");
                (
                    runtime.block_on(valid.test_connection()),
                    runtime.block_on(embedded.test_connection()),
                )
            });
        server.join().expect("SOCKS listener thread panicked");

        assert!(
            handshake_result
                .steps
                .iter()
                .any(|step| step.name == "SOCKS5" && !step.passed),
            "failed SOCKS handshake was not reported: {handshake_result:?}"
        );
        assert!(
            embedded_result.steps.iter().any(|step| !step.passed),
            "invalid embedded userinfo was not reported: {embedded_result:?}"
        );

        let diagnostics = format!(
            "{}\n{handshake_result:?}\n{embedded_result:?}",
            capture.output()
        );
        assert_credentials_redacted(&diagnostics);
        assert!(diagnostics.contains("<invalid address>"));
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
    fn storage_validation_rejects_disabled_nonempty_address_userinfo_without_echoing_it() {
        let mut config = embedded_userinfo_config();
        config.enabled = false;

        let error = config
            .validate_for_storage()
            .expect_err("stored nonempty proxy authorities must remain strict when disabled");

        assert_credentials_redacted(&error);
        assert!(error.contains("<invalid address>"), "{error}");
    }

    #[test]
    fn storage_validation_accepts_blank_disabled_proxy() {
        let config = ProxyConfig {
            enabled: false,
            address: String::new(),
            username: Some(USERNAME_SECRET.to_string()),
            password: Some(PASSWORD_SECRET.to_string()),
        };

        assert!(config.validate_for_storage().is_ok());
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
