//! Settings action handlers
//!
//! Extracted from ui.rs to reduce file size and improve organization.

use crate::core::navigation::SettingsPage;
use crate::features::plugins::domain::types::PluginsListState;

use crate::features::settings::domain::types::{
    ArchivesSettingsState, SecuritySettingsState, ServerSettingsState, SettingsAction,
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
    network_state: &mut crate::features::settings::domain::types::NetworkSettingsState,
    server_state: &mut ServerSettingsState,

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
            // Save via ConfigService if available
            if let Some(ref config_svc) = shared.services.config_service {
                let res: anyhow::Result<()> = config_svc.save_user_config(&state.user_config);
                if let Err(e) = res {
                    tracing::error!("Failed to save user config: {}", e);
                }
            }
        }
        SettingsAction::SaveKeyboardMouse { bindings } => {
            let mut state = shared.app_state.lock();
            // Update user config - bindings is HashMap, storing as JSON string
            state.user_config.hotkey_bindings = serde_json::to_string(&bindings).ok();
            state.signals.user_config.set(state.user_config.clone());

            // Save to DB via ConfigService
            if let Some(ref config_svc) = shared.services.config_service {
                if let Err(e) = config_svc.save_user_config(&state.user_config) {
                    tracing::error!("Failed to save hotkey bindings: {}", e);
                } else {
                    tracing::info!("Hotkey bindings saved successfully");
                }
            }

            // Signal app to reload hotkeys
            state.signals.hotkeys_updated.set(true);
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
        SettingsAction::GarbageCollectCache => {
            let mut state = shared.app_state.lock();
            if let Some(dbs) = &mut state.dbs {
                match dbs.metadata.delete_orphaned_cache_entries() {
                    Ok(count) => {
                        tracing::info!("Garbage collected {} orphaned cache entries", count);
                    }
                    Err(e) => {
                        tracing::error!("Failed to garbage collect cache: {}", e);
                    }
                }
            }
        }
        SettingsAction::CleanOldSearchCache => {
            let mut state = shared.app_state.lock();
            if let Some(dbs) = &mut state.dbs {
                match dbs.metadata.delete_old_search_cache(7) {
                    Ok(count) => {
                        tracing::info!("Cleaned {} old search cache entries", count);
                    }
                    Err(e) => {
                        tracing::error!("Failed to clean search cache: {}", e);
                    }
                }
            }
        }
        SettingsAction::MigrateCacheEntries => {
            let mut state = shared.app_state.lock();
            if let Some(dbs) = &mut state.dbs {
                match dbs.metadata.migrate_fix_cache_entries() {
                    Ok((type_fixed, product_fixed)) => {
                        tracing::info!(
                            "Fixed cache entries: {} cache_type, {} product_id",
                            type_fixed,
                            product_fixed
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to migrate cache entries: {}", e);
                    }
                }
            }
        }
        SettingsAction::SaveGeneral {
            open_nested_in_new_tab,
        } => {
            let mut state = shared.app_state.lock();
            state.user_config.open_nested_in_new_tab = open_nested_in_new_tab;
            state.signals.user_config.set(state.user_config.clone());
            if let Some(ref config_svc) = shared.services.config_service {
                if let Err(e) = config_svc.save_user_config(&state.user_config) {
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
            // Sanitize address: strip any protocol prefixes
            let clean_address = socks5_address.map(|addr| {
                addr.strip_prefix("socks5h://")
                    .or_else(|| addr.strip_prefix("socks5://"))
                    .or_else(|| addr.strip_prefix("http://"))
                    .or_else(|| addr.strip_prefix("https://"))
                    .unwrap_or(&addr)
                    .to_string()
            });

            let mut state = shared.app_state.lock();
            state.user_config.socks5_enabled = socks5_enabled;
            state.user_config.socks5_address = clean_address.clone();
            state.user_config.socks5_username = socks5_username.clone();
            state.signals.user_config.set(state.user_config.clone());

            let mut password_to_use = None;

            // Save config via ConfigService
            if let Some(ref config_svc) = shared.services.config_service {
                match config_svc.save_user_config(&state.user_config) {
                    Ok(_) => {
                        tracing::info!("[SaveNetwork] Network settings saved successfully: enabled={}, address={:?}", 
                            socks5_enabled, clean_address);
                    }
                    Err(e) => {
                        tracing::error!("[SaveNetwork] Failed to save network settings: {}", e);
                    }
                }
            }

            // Handle password via secrets DB (not yet in ConfigService)
            if let Some(ref dbs) = state.dbs {
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

            // Update client. Validate the config first so an invalid
            // SOCKS5 address surfaces as a toast rather than silently
            // disabling the proxy (audit finding M4).
            use arclain_network::features::proxy::ProxyConfig;
            let config = ProxyConfig {
                enabled: socks5_enabled,
                address: clean_address.unwrap_or_default(),
                username: socks5_username,
                password: password_to_use,
            };

            if let Err(msg) = config.validate() {
                tracing::warn!("[SaveNetwork] Refusing to apply invalid proxy: {}", msg);
                shared.toaster.lock().error(format!(
                    "Network settings saved, but proxy is disabled: {}",
                    msg,
                ));
                // Apply with proxy_config = None so we don't ship a half-broken
                // client to the rest of the app.
                shared.services.async_http_client.update_config(None);
            } else {
                shared
                    .services
                    .async_http_client
                    .update_config(Some(config));
                tracing::info!("Network settings saved");
            }
        }
        SettingsAction::TestNetwork {
            socks5_enabled,
            socks5_address,
            socks5_username,
            socks5_password,
        } => {
            use crate::features::settings::domain::types::{
                ConnectionTestResult, ConnectionTestStatus, TestStepResult,
            };

            // Set testing state
            network_state
                .connection_test_status
                .set(ConnectionTestStatus::Testing);
            let status_signal = network_state.connection_test_status.clone();

            // Create config for test
            use arclain_network::features::proxy::ProxyConfig;
            let config = ProxyConfig {
                enabled: socks5_enabled,
                address: socks5_address.unwrap_or_default(),
                username: socks5_username,
                password: socks5_password,
            };

            // Spawn test task
            let runtime = shared.services.tokio_runtime.handle().clone();
            runtime.spawn(async move {
                let network_result = config.test_connection().await;

                // Convert network types to UI types
                let ui_result = ConnectionTestResult {
                    steps: network_result
                        .steps
                        .into_iter()
                        .map(|s| TestStepResult {
                            name: s.name,
                            passed: s.passed,
                            message: s.message,
                        })
                        .collect(),
                    success: network_result.success,
                    result_message: if network_result.success {
                        Some(format!(
                            "{} ({})",
                            network_result.ip.unwrap_or_default(),
                            network_result.country.unwrap_or_default()
                        ))
                    } else {
                        None
                    },
                };

                status_signal.set(ConnectionTestStatus::Complete(ui_result));
            });
        }
        SettingsAction::SaveServer {
            enabled,
            url,
            api_key,
        } => {
            let mut state = shared.app_state.lock();
            state.user_config.gameta_server_enabled = enabled;
            state.user_config.gameta_server_url = url.clone();
            state.signals.user_config.set(state.user_config.clone());

            if let Some(ref config_svc) = shared.services.config_service {
                match config_svc.save_user_config(&state.user_config) {
                    Ok(_) => {
                        tracing::info!(
                            "[SaveServer] Server settings saved: enabled={}, url={:?}",
                            enabled,
                            url
                        );
                    }
                    Err(e) => {
                        tracing::error!("[SaveServer] Failed to save server settings: {}", e);
                    }
                }
            }

            // Persist API key in secrets DB
            if let Some(ref dbs) = state.dbs {
                if let Some(key) = &api_key {
                    if let Err(e) = dbs.secrets.set_secret("gameta:api_key", key) {
                        tracing::error!("[SaveServer] Failed to save API key: {}", e);
                    }
                }
            }
        }
        SettingsAction::TestServer { url, api_key } => {
            use crate::features::settings::domain::types::ServerConnectionStatus;

            server_state
                .connection_status
                .set(ServerConnectionStatus::Testing);
            let status_signal = server_state.connection_status.clone();

            // GametaClient is blocking — run on a dedicated thread to avoid
            // blocking the egui render loop.
            std::thread::spawn(move || {
                use arclain_network::features::gameta_client::{GametaClient, ServerConfig};

                let client = GametaClient::new(ServerConfig {
                    url: url.clone(),
                    api_key: api_key.clone(),
                });

                match client.health() {
                    Ok(resp) => {
                        let msg = format!(
                            "gameta server v{} is reachable",
                            resp.version
                        );
                        status_signal.set(ServerConnectionStatus::Connected(msg));
                    }
                    Err(e) => {
                        status_signal.set(ServerConnectionStatus::Failed(e));
                    }
                }
            });
        }
        SettingsAction::NavigateTo(_) => {
            // Navigation is handled by extract_navigation before this function is called
        }
        SettingsAction::SaveEditedRule => {
            // Handled specially in SettingsFeature::render where rules_page is available
        }
    }
}
