//! Proxy configuration utilities
//!
//! Handles resolution of proxy settings from UserConfig and SecretsDb,
//! and application to the AsyncHttpClient.

use arclain_db::{SecretsDb, UserConfig};
use arclain_network::{features::proxy::ProxyConfig, AsyncHttpClient};
use std::collections::HashMap;

/// Resolve proxy configuration from UserConfig and SecretsDb
pub fn resolve_proxy_config(user_config: &UserConfig, secrets: &SecretsDb) -> Option<ProxyConfig> {
    if !user_config.socks5_enabled {
        return None;
    }

    let address = user_config.socks5_address.clone().unwrap_or_default();

    if address.is_empty() {
        return None;
    }

    let mut password = None;
    if user_config.socks5_username.is_some() {
        // Try to load password from secrets
        if let Ok(Some(pwd)) = secrets.get_secret("proxy:socks5") {
            let pwd_str: &str = pwd.as_ref();
            password = Some(pwd_str.to_string());
        }
    }

    Some(ProxyConfig {
        enabled: true,
        address,
        username: user_config.socks5_username.clone(),
        password,
    })
}

/// Apply proxy configuration to the HTTP client
pub fn apply_proxy_to_client(
    client: &AsyncHttpClient,
    proxy_config: Option<ProxyConfig>,
    user_config: &UserConfig,
) {
    if let Some(config) = proxy_config {
        if !config.enabled {
            tracing::info!("[Proxy] {}", config.log_summary());
            client.update_config(None);
            client.update_plugin_proxy_map(HashMap::new());
            return;
        }
        if let Err(error) = config.validate() {
            tracing::warn!("[Proxy] Refusing invalid proxy configuration: {}", error);
            client.update_config(None);
            client.update_plugin_proxy_map(HashMap::new());
            return;
        }

        tracing::info!("[Proxy] Enabling {}", config.log_summary());
        client.update_config(Some(config));

        // Enable proxy for specific plugins if configured
        let mut proxy_map = user_config.get_plugin_proxy_settings();

        // Ensure DLSite variants are covered by default if not strictly disabled?
        // Current logic enforces them if absent.
        if !proxy_map.contains_key("dlsite") {
            proxy_map.insert("dlsite".to_string(), true);
        }
        if !proxy_map.contains_key("dlsite-api") {
            proxy_map.insert("dlsite-api".to_string(), true);
        }
        if !proxy_map.contains_key("dlsite-html") {
            proxy_map.insert("dlsite-html".to_string(), true);
        }

        client.update_plugin_proxy_map(proxy_map);
    } else {
        tracing::info!("[Proxy] SOCKS5 proxy disabled");
        client.update_config(None);
        client.update_plugin_proxy_map(HashMap::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::fmt::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    #[derive(Clone, Default)]
    struct EventCapture {
        events: Arc<Mutex<Vec<String>>>,
        next_span: Arc<AtomicU64>,
    }

    impl EventCapture {
        fn output(&self) -> String {
            self.events.lock().unwrap().join("\n")
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
            struct Visitor(String);
            impl Visit for Visitor {
                fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
                    write!(&mut self.0, "{}={value:?} ", field.name()).unwrap();
                }
            }

            let mut visitor = Visitor(String::new());
            event.record(&mut visitor);
            self.events.lock().unwrap().push(visitor.0);
        }

        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    #[test]
    fn invalid_proxy_address_is_not_logged_or_applied() {
        const ADDRESS_USER: &str = "core-address-user-secret-38af";
        const ADDRESS_PASSWORD: &str = "core-address-password-secret-74bc";
        const DIRECT_USER: &str = "core-direct-user-secret-12de";
        const DIRECT_PASSWORD: &str = "core-direct-password-secret-56f0";

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = AsyncHttpClient::new(
            runtime.handle().clone(),
            Arc::new(parking_lot::RwLock::new(
                arclain_network::features::whitelist::DomainWhitelist::default(),
            )),
            None,
        );
        let config = ProxyConfig {
            enabled: true,
            address: format!("{ADDRESS_USER}:{ADDRESS_PASSWORD}@proxy.example:1080"),
            username: Some(DIRECT_USER.to_string()),
            password: Some(DIRECT_PASSWORD.to_string()),
        };
        let capture = EventCapture::default();

        tracing::subscriber::with_default(capture.clone(), || {
            apply_proxy_to_client(&client, Some(config), &UserConfig::default());
        });

        let diagnostics = capture.output();
        for secret in [ADDRESS_USER, ADDRESS_PASSWORD, DIRECT_USER, DIRECT_PASSWORD] {
            assert!(
                !diagnostics.contains(secret),
                "proxy secret leaked in {diagnostics:?}"
            );
        }
        assert!(diagnostics.contains("<invalid address>"), "{diagnostics}");
        assert!(!client.should_use_proxy_for_plugin("dlsite"));
    }
}
