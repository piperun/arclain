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

    let mut address = user_config.socks5_address.clone().unwrap_or_default();
    // Strip protocol if present for consistency
    if let Some(stripped) = address
        .strip_prefix("socks5://")
        .or_else(|| address.strip_prefix("socks5h://"))
    {
        address = stripped.to_string();
    }

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
        tracing::info!("[Proxy] Enabling SOCKS5 proxy at {}", config.address);
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
