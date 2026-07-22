//! Proxy configuration utilities
//!
//! Handles resolution of proxy settings from UserConfig and SecretsDb,
//! and application to the AsyncHttpClient.

use anyhow::{anyhow, Context, Result};
use arclain_db::{SecretsDb, UserConfig};
use arclain_network::{features::proxy::ProxyConfig, AsyncHttpClient};
use std::collections::HashMap;

const DEFAULT_PROXIED_PLUGINS: [&str; 4] =
    ["dlsite-metadata", "dlsite", "dlsite-api", "dlsite-html"];

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
pub fn resolve_proxy_config(
    user_config: &UserConfig,
    secrets: &SecretsDb,
) -> Result<Option<ProxyConfig>> {
    if !user_config.socks5_enabled {
        return Ok(None);
    }

    let address = user_config.socks5_address.clone().unwrap_or_default();

    if address.is_empty() {
        return Ok(None);
    }

    let mut password = None;
    if user_config.socks5_username.is_some() {
        if let Some(pwd) = secrets
            .get_secret("proxy:socks5")
            .context("loading proxy password from encrypted storage")?
        {
            let pwd_str: &str = pwd.as_ref();
            password = Some(pwd_str.to_string());
        }
    }

    Ok(Some(ProxyConfig {
        enabled: true,
        address,
        username: user_config.socks5_username.clone(),
        password,
    }))
}

/// Apply proxy configuration to the HTTP client
pub fn apply_proxy_to_client(
    client: &AsyncHttpClient,
    proxy_config: Option<ProxyConfig>,
    user_config: &UserConfig,
) -> Result<()> {
    if !user_config.socks5_enabled {
        tracing::info!("[Proxy] SOCKS5 proxy disabled");
        client.apply_proxy_routing(None, effective_plugin_proxy_map(user_config));
        return Ok(());
    }

    let Some(config) = proxy_config else {
        client.mark_plugin_routing_unavailable();
        tracing::warn!(
            "[Proxy] Enabled SOCKS5 transport is unavailable; plugin requests will fail closed"
        );
        return Err(anyhow!("enabled SOCKS5 transport is unavailable"));
    };
    if !config.enabled {
        client.mark_plugin_routing_unavailable();
        tracing::warn!("[Proxy] Refusing disabled transport for enabled proxy settings");
        return Err(anyhow!(
            "enabled proxy settings resolved to a disabled transport"
        ));
    }
    if let Err(error) = config.validate() {
        client.mark_plugin_routing_unavailable();
        tracing::warn!("[Proxy] Refusing invalid proxy configuration: {}", error);
        return Err(anyhow!(error));
    }

    tracing::info!("[Proxy] Enabling {}", config.log_summary());
    client.apply_proxy_routing(Some(config), effective_plugin_proxy_map(user_config));
    Ok(())
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
            apply_proxy_to_client(&client, Some(config), &user_config)
                .expect_err("invalid proxy must fail closed");
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

        apply_proxy_to_client(&client, None, &user_config)
            .expect_err("missing enabled proxy must fail closed");

        assert!(!client.should_use_proxy_for_plugin("dlsite"));
        assert!(!client.should_use_proxy_for_plugin("dlsite-metadata"));
        assert!(!client.should_use_proxy_for_plugin("custom"));
        assert!(!client.should_use_proxy_for_plugin("dlsite-api"));
    }

    #[test]
    fn valid_proxy_reapply_clears_unavailable_checked_routing_state() {
        const PLUGIN_ID: &str = "dlsite-metadata";
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

        apply_proxy_to_client(&client, None, &user_config)
            .expect_err("missing enabled proxy must fail closed");
        assert!(!client.should_use_proxy_for_plugin(PLUGIN_ID));

        let valid = ProxyConfig {
            enabled: true,
            address: "127.0.0.1:1080".to_string(),
            username: None,
            password: None,
        };
        apply_proxy_to_client(&client, Some(valid), &user_config)
            .expect("valid proxy must restore checked routing");
        assert!(client.should_use_proxy_for_plugin(PLUGIN_ID));
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
        assert_eq!(enabled.get("dlsite-metadata"), Some(&true));

        user_config.set_plugin_proxy_enabled("dlsite-metadata", false);
        assert_eq!(
            effective_plugin_proxy_map(&user_config).get("dlsite-metadata"),
            Some(&false)
        );

        user_config.socks5_enabled = false;
        assert!(effective_plugin_proxy_map(&user_config).is_empty());
    }
}
