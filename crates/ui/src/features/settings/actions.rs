//! Settings action handlers
//!
//! Extracted from ui.rs to reduce file size and improve organization.

use crate::core::navigation::SettingsPage;
use crate::features::plugins::types::PluginsListState;
use crate::features::settings::settings_content::{
    ArchivesSettingsState, SecuritySettingsState, SettingsAction,
};
use crate::shared::SharedState;

/// Check if action is navigation and extract target page
pub fn extract_navigation(action: &SettingsAction) -> Option<SettingsPage> {
    match action {
        SettingsAction::NavigateTo(page) => Some(page.clone()),
        _ => None,
    }
}

/// Handle a settings action, mutating the appropriate state
pub fn handle_action(
    action: SettingsAction,
    security_state: &mut SecuritySettingsState,
    archives_state: &mut ArchivesSettingsState,
    plugins_state: &mut PluginsListState,
    network_state: &mut crate::features::settings::types::NetworkSettingsState,
    shared: &SharedState,
) {
    match action {
        SettingsAction::SaveSecurity {
            key_file_path,
            secrets_db_path,
            encrypted_crc_policy,
        } => {
            let mut state = shared.app_state.lock();
            let key_file_str = key_file_path;
            let secrets_db_str = secrets_db_path;

            if let Err(e) = state.apply_preferences(
                key_file_str,
                secrets_db_str,
                encrypted_crc_policy,
                shared.services.plugin_manager.as_ref(),
            ) {
                *security_state.error.write() = format!("Failed to save settings: {}", e);
            } else {
                *security_state.info.write() = "Settings saved successfully".to_string();
            }
        }
        SettingsAction::SaveArchives { temp_dir } => {
            let mut state = shared.app_state.lock();
            state.user_config.temp_dir = temp_dir;
            state.signals.user_config.set(state.user_config.clone());
            // Save via DB if available
            if let Some(ref dbs) = state.dbs {
                let _ = dbs.config.with_connection(|conn| {
                    state.user_config.save(conn).ok();
                    Ok::<_, anyhow::Error>(())
                });
            }
        }
        SettingsAction::MoveVault { dest_path } => {
            let mut state = shared.app_state.lock();
            if let Err(e) = state.move_vault(&dest_path, shared.services.plugin_manager.as_ref()) {
                *security_state.error.write() = format!("Failed to move vault: {}", e);
            } else {
                *security_state.info.write() = "Vault moved successfully".to_string();
            }
        }
        SettingsAction::RekeyVault { new_key_file_path } => {
            let mut state = shared.app_state.lock();
            if let Err(e) =
                state.rekey_vault(&new_key_file_path, shared.services.plugin_manager.as_ref())
            {
                *security_state.error.write() = format!("Failed to rekey vault: {}", e);
            } else {
                *security_state.info.write() = "Vault rekeyed successfully".to_string();
            }
        }
        SettingsAction::SavePasswordRules { rules } => {
            let mut state = shared.app_state.lock();
            let core_rules = rules
                .into_iter()
                .map(|r| arclain_core::PassRule {
                    name: r.name,
                    pattern: r.pattern,
                    password: r.password,
                    priority: r.priority,
                    enabled: r.enabled,
                })
                .collect();
            if let Err(e) = state.save_password_rules(core_rules) {
                tracing::error!("Failed to save password rules: {}", e);
            }
        }
        SettingsAction::InstallPlugin { wasm_path } => {
            // Get plugin_manager from services (no lock needed)
            if let Some(manager) = &shared.services.plugin_manager {
                let mut mgr = manager.lock();
                match mgr.install_plugin(std::path::Path::new(&wasm_path)) {
                    Ok(id) => {
                        tracing::info!("Successfully installed plugin: {}", id);
                        // Refresh list
                        let state = shared.app_state.lock();
                        plugins_state.update_from_manager(&mgr, &state.user_config);
                    }
                    Err(e) => {
                        tracing::error!("Failed to install plugin: {}", e);
                    }
                }
            }
        }
        SettingsAction::ClearCacheIndex => {
            let mut state = shared.app_state.lock();
            if let Some(dbs) = &mut state.dbs {
                if let Err(e) = dbs.metadata.clear_cache_index() {
                    *archives_state.checksum_enabled.write() = false;
                    tracing::error!("Failed to clear cache index: {}", e);
                } else {
                    tracing::info!("Cache index cleared successfully");
                }
            }
        }
        SettingsAction::ClearCacheContent => {
            let state = shared.app_state.lock();
            let cache_dir = if let Some(paths) = &state.db_paths {
                paths
                    .cache_db
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join("content")
            } else {
                std::path::PathBuf::from("data/content")
            };
            drop(state);

            std::thread::spawn(move || {
                tracing::info!("Clearing cache content at {:?}", cache_dir);
                if cache_dir.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
                        tracing::error!("Failed to remove cache dir: {}", e);
                    }
                    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                        tracing::error!("Failed to recreate cache dir: {}", e);
                    }
                }
            });
        }
        SettingsAction::SaveGeneral {
            open_nested_in_new_tab,
        } => {
            let mut state = shared.app_state.lock();
            state.user_config.open_nested_in_new_tab = open_nested_in_new_tab;
            state.signals.user_config.set(state.user_config.clone());
            if let Some(ref dbs) = state.dbs {
                if let Err(e) = dbs.config.with_connection(|conn| {
                    state.user_config.save(conn).ok();
                    Ok::<_, anyhow::Error>(())
                }) {
                    tracing::error!("Failed to save general settings: {}", e);
                } else {
                    tracing::info!("General settings saved");
                }
            }
        }
        SettingsAction::SaveNetwork {
            socks5_enabled,
            socks5_address,
            socks5_username,
            socks5_password,
        } => {
            let mut state = shared.app_state.lock();
            state.user_config.socks5_enabled = socks5_enabled;
            state.user_config.socks5_address = socks5_address.clone();
            state.user_config.socks5_username = socks5_username.clone();
            state.signals.user_config.set(state.user_config.clone());

            let mut password_to_use = None;

            if let Some(ref dbs) = state.dbs {
                // Save config
                match dbs
                    .config
                    .with_connection(|conn| Ok::<_, anyhow::Error>(state.user_config.save(conn)))
                {
                    Ok(_) => {
                        tracing::info!("[SaveNetwork] Network settings saved successfully: enabled={}, address={:?}", 
                            socks5_enabled, socks5_address);
                    }
                    Err(e) => {
                        tracing::error!("[SaveNetwork] Failed to save network settings: {}", e);
                    }
                }

                // Handle password
                if let Some(pwd) = &socks5_password {
                    if let Err(e) = dbs.secrets.set_secret("proxy:socks5", pwd) {
                        tracing::error!("Failed to save proxy password: {}", e);
                    }
                    password_to_use = Some(pwd.clone());
                } else {
                    // Try to load existing
                    if let Ok(Some(existing)) = dbs.secrets.get_secret("proxy:socks5") {
                        password_to_use = Some(existing.to_string());
                    }
                }
            }

            // Update client
            use arclain_http::features::proxy::ProxyConfig;
            let config = ProxyConfig {
                enabled: socks5_enabled,
                address: socks5_address.unwrap_or_default(),
                username: socks5_username,
                password: password_to_use,
            };

            shared
                .services
                .async_http_client
                .update_config(Some(config));
            tracing::info!("Network settings saved");
        }
        SettingsAction::TestNetwork {
            socks5_enabled,
            socks5_address,
            socks5_username,
            socks5_password,
        } => {
            use crate::features::settings::types::ConnectionTestStatus;

            // Set testing state
            network_state
                .connection_test_status
                .set(ConnectionTestStatus::Testing);
            let status_signal = network_state.connection_test_status.clone();

            // Create config for test
            use arclain_http::features::proxy::ProxyConfig;
            let config = ProxyConfig {
                enabled: socks5_enabled,
                address: socks5_address.unwrap_or_default(),
                username: socks5_username,
                password: socks5_password,
            };

            // Spawn test task
            let runtime = shared.services.tokio_runtime.handle().clone();
            runtime.spawn(async move {
                let result = async {
                    // Build client manually since we want to test specific config without affecting global state
                    let mut builder = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(10))
                        .timeout(std::time::Duration::from_secs(10));

                    if let Some(proxy) = config.to_proxy() {
                        builder = builder.proxy(proxy);
                    } else if config.enabled {
                        return Err(anyhow::anyhow!("Invalid proxy configuration"));
                    }

                    let client = builder
                        .build()
                        .map_err(|e| anyhow::anyhow!("Failed to build client: {}", e))?;

                    // Test connection to a reliable endpoint
                    // We use Google as a generic connectivity check, or maybe Cloudflare
                    let _ = client
                        .get("https://www.google.com")
                        .send()
                        .await
                        .map_err(|e| anyhow::anyhow!("Connection failed: {}", e))?
                        .error_for_status()
                        .map_err(|e| anyhow::anyhow!("HTTP error: {}", e))?;

                    Ok::<_, anyhow::Error>("Connection successful".to_string())
                }
                .await;

                match result {
                    Ok(msg) => status_signal.set(ConnectionTestStatus::Success(msg)),
                    Err(e) => status_signal.set(ConnectionTestStatus::Error(e.to_string())),
                }
            });
        }
        SettingsAction::NavigateTo(_) => {
            // Navigation is handled by extract_navigation before this function is called
        }
    }
}
