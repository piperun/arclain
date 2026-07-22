//! Proxy configuration utilities
//!
//! Handles resolution of proxy settings from UserConfig and SecretsDb,
//! and application to the AsyncHttpClient.

use arclain_db::{SecretsDb, UserConfig};
use arclain_network::{features::proxy::ProxyConfig, AsyncHttpClient};
use std::collections::HashMap;

const DEFAULT_PROXIED_PLUGINS: [&str; 3] = ["dlsite", "dlsite-api", "dlsite-html"];

/// Derive the runtime routing map from persisted settings.
///
/// DLSite integrations use the global proxy by default, while an explicit
/// persisted `false` remains an opt-out. A disabled global proxy always
/// clears every per-plugin route.
pub fn effective_plugin_proxy_map(user_config: &UserConfig) -> HashMap<String, bool> {
    if !user_config.socks5_enabled {
        return HashMap::new();
    }

    let mut proxy_map = user_config.get_plugin_proxy_settings();
    for plugin_id in DEFAULT_PROXIED_PLUGINS {
        proxy_map.entry(plugin_id.to_string()).or_insert(true);
    }
    proxy_map
}

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

fn clear_proxy_transport(client: &AsyncHttpClient, user_config: &UserConfig) {
    client.update_config(None);
    client.update_plugin_proxy_map(effective_plugin_proxy_map(user_config));
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
            clear_proxy_transport(client, user_config);
            return;
        }
        if let Err(error) = config.validate() {
            tracing::warn!("[Proxy] Refusing invalid proxy configuration: {}", error);
            clear_proxy_transport(client, user_config);
            return;
        }

        tracing::info!("[Proxy] Enabling {}", config.log_summary());
        client.update_config(Some(config));

        client.update_plugin_proxy_map(effective_plugin_proxy_map(user_config));
    } else {
        if user_config.socks5_enabled {
            tracing::warn!(
                "[Proxy] Enabled SOCKS5 transport is unavailable; proxied plugin routes will fail closed"
            );
        } else {
            tracing::info!("[Proxy] SOCKS5 proxy disabled");
        }
        clear_proxy_transport(client, user_config);
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
    fn invalid_enabled_proxy_is_redacted_and_keeps_plugin_routing_fail_closed() {
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

        let mut user_config = UserConfig::default();
        user_config.socks5_enabled = true;

        tracing::subscriber::with_default(capture.clone(), || {
            apply_proxy_to_client(&client, Some(config), &user_config);
        });

        let diagnostics = capture.output();
        for secret in [ADDRESS_USER, ADDRESS_PASSWORD, DIRECT_USER, DIRECT_PASSWORD] {
            assert!(
                !diagnostics.contains(secret),
                "proxy secret leaked in {diagnostics:?}"
            );
        }
        assert!(diagnostics.contains("<invalid address>"), "{diagnostics}");
        assert!(client.should_use_proxy_for_plugin("dlsite"));
    }

    #[test]
    fn missing_enabled_proxy_transport_keeps_plugin_routing_fail_closed() {
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
        let mut user_config = UserConfig::default();
        user_config.socks5_enabled = true;
        user_config.set_plugin_proxy_enabled("custom", true);
        user_config.set_plugin_proxy_enabled("dlsite-api", false);

        apply_proxy_to_client(&client, None, &user_config);

        assert!(client.should_use_proxy_for_plugin("dlsite"));
        assert!(client.should_use_proxy_for_plugin("custom"));
        assert!(!client.should_use_proxy_for_plugin("dlsite-api"));
    }

    #[test]
    fn effective_proxy_map_applies_defaults_and_preserves_explicit_overrides() {
        let mut user_config = UserConfig::default();
        user_config.socks5_enabled = true;
        user_config.set_plugin_proxy_enabled("custom", true);
        user_config.set_plugin_proxy_enabled("dlsite-api", false);

        let enabled = effective_plugin_proxy_map(&user_config);
        assert_eq!(enabled.get("custom"), Some(&true));
        assert_eq!(enabled.get("dlsite"), Some(&true));
        assert_eq!(enabled.get("dlsite-api"), Some(&false));
        assert_eq!(enabled.get("dlsite-html"), Some(&true));

        user_config.socks5_enabled = false;
        assert!(effective_plugin_proxy_map(&user_config).is_empty());
    }
}
