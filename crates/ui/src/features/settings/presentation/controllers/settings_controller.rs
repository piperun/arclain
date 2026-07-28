//! Settings action handlers
//!
//! Extracted from ui.rs to reduce file size and improve organization.

use crate::core::navigation::SettingsPage;
use crate::features::plugins::domain::types::PluginsListState;

use crate::features::settings::domain::types::{
    ArchivesSettingsState, SecuritySettingsState, ServerSettingsState, SettingsAction,
};
use arclain_core::services::{NetworkProxyPersistenceService, ProxySaveOutcome};
use arclain_core::utilities::effective_plugin_proxy_map;
use arclain_network::features::proxy::ProxyConfig;

use crate::shared::SharedState;

/// Check if action is navigation and extract target page
pub fn extract_navigation(action: &SettingsAction) -> Option<SettingsPage> {
    match action {
        SettingsAction::NavigateTo(page) => Some(page.clone()),
        _ => None,
    }
}

fn log_saved_proxy_configuration(config: &ProxyConfig) {
    tracing::info!(
        "[SaveNetwork] Network settings saved successfully: {}",
        config.log_summary()
    );
}

/// Handle a settings action, mutating the appropriate state.
///
/// `plugins_state` remains optional because most call sites don't carry
/// the PluginsFeature borrow (for example, password-rule dialogs).
pub fn handle_action(
    action: SettingsAction,
    security_state: &mut SecuritySettingsState,
    archives_state: &mut ArchivesSettingsState,
    _plugins_state: Option<&mut PluginsListState>,
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
            shared.plugin_ui_jobs.request(
                crate::features::plugins::application::PluginUiRequest::Install {
                    wasm_path: std::path::PathBuf::from(wasm_path),
                },
            );
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
                tracing::info!("Clearing cache content at {}", cache_dir.display());
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
            drop_behavior,
            restore_tabs_on_launch,
        } => {
            let mut state = shared.app_state.lock();
            state.user_config.open_nested_in_new_tab = open_nested_in_new_tab;
            state.user_config.drop_behavior = Some(drop_behavior.as_str().to_string());
            state.user_config.restore_tabs_on_launch = restore_tabs_on_launch;
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
            let config = ProxyConfig {
                enabled: socks5_enabled,
                address: socks5_address.clone().unwrap_or_default(),
                username: socks5_username.clone(),
                password: socks5_password.filter(|password| !password.is_empty()),
            };
            if let Err(message) = config.validate_for_storage() {
                tracing::warn!(
                    "[SaveNetwork] Refusing to save invalid proxy configuration: {}",
                    message
                );
                shared
                    .toaster
                    .lock()
                    .error(format!("Network settings were not saved: {message}"));
                return;
            }

            let Some(config_svc) = shared.services.config_service.as_ref() else {
                tracing::error!("[SaveNetwork] ConfigService is unavailable");
                shared
                    .toaster
                    .lock()
                    .error("Network settings were not saved: configuration storage is unavailable");
                return;
            };

            let mut state = shared.app_state.lock();
            let mut candidate = state.user_config.clone();
            candidate.socks5_enabled = socks5_enabled;
            candidate.socks5_address = socks5_address;
            candidate.socks5_username = socks5_username;

            let save_result = {
                let Some(dbs) = state.dbs.as_ref() else {
                    tracing::error!("[SaveNetwork] SecretsDb is unavailable");
                    shared.toaster.lock().error(
                        "Network settings were not saved: encrypted secrets storage is unavailable",
                    );
                    return;
                };
                NetworkProxyPersistenceService::new(config_svc, &dbs.secrets)
                    .save(&candidate, config.password.as_deref())
            };
            let outcome = match save_result {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::error!("[SaveNetwork] Failed to save network settings: {error}");
                    shared
                        .toaster
                        .lock()
                        .error("Network settings were not saved: configuration persistence failed");
                    return;
                }
            };

            if outcome == ProxySaveOutcome::CommittedPendingFinalize {
                tracing::warn!(
                    "[SaveNetwork] Network settings committed; journal cleanup will retry at startup"
                );
            }

            let plugin_proxy_map = effective_plugin_proxy_map(&candidate);
            state.user_config = candidate;
            state.signals.user_config.set(state.user_config.clone());
            drop(state);

            shared
                .services
                .async_http_client
                .apply_proxy_routing(Some(config.clone()), plugin_proxy_map);
            log_saved_proxy_configuration(&config);
            tracing::info!("Network settings saved");
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
                        let msg = format!("gameta server v{} is reachable", resp.version);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::services::Services;
    use crate::core::signals::AppSignals;
    use crate::core::state::AppState;
    use crate::shared::theme::AppTheme;
    use arclain_core::backends::sevenz_cli::SevenZipCli;
    use arclain_core::backends::BackendSelector;
    use arclain_core::services::ConfigService;
    use arclain_core::{open_databases, DbConnection, DbPaths, SecretsKey, UserConfig};
    use arclain_network::features::proxy::ProxyConfig;
    use arclain_widgets::Toaster;
    use parking_lot::Mutex;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tracing_test::traced_test;

    const PROXY_PLUGIN_ID: &str = "proxy-map-test-plugin";

    fn serve_proxy_sentinel(
        proxy: TcpListener,
        request_finished: Arc<AtomicBool>,
        reached_proxy: Arc<AtomicBool>,
    ) {
        // The HTTP request timeout bounds this lifecycle. A shorter wall-clock
        // deadline makes the sentinel depend on when the OS schedules its thread.
        while !request_finished.load(Ordering::SeqCst) {
            match proxy.accept() {
                Ok((mut socket, _)) => {
                    reached_proxy.store(true, Ordering::SeqCst);
                    let _ = socket.write_all(&[0x05, 0xff]);
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("proxy sentinel accept failed: {error}"),
            }
        }
    }

    struct ProxySaveFixture {
        shared: SharedState,
        config_service: Arc<ConfigService>,
        previous: UserConfig,
        previous_password: String,
        proxy_listener: TcpListener,
        _temp: TempDir,
    }

    impl ProxySaveFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("create proxy settings test directory");
            let paths = DbPaths {
                config_db: temp.path().join("config.sqlite"),
                cache_db: temp.path().join("metadata.sqlite"),
                secrets_db: temp.path().join("pass.redb"),
                key_file: None,
            };
            let dbs = open_databases(&paths, &SecretsKey::generate())
                .expect("open proxy settings test databases");
            let config_connection =
                DbConnection::open(&paths.config_db).expect("open config connection");
            UserConfig::ensure_table(&config_connection).expect("migrate user config schema");
            let config_service = Arc::new(ConfigService::from_connection(
                dbs.config_pool.clone(),
                config_connection,
            ));

            let proxy_listener =
                TcpListener::bind("127.0.0.1:0").expect("reserve previous proxy address");
            let proxy_address = proxy_listener
                .local_addr()
                .expect("read previous proxy address");
            let previous_password = "previous-proxy-password-4e7a".to_string();
            let mut previous = UserConfig::new();
            previous.socks5_enabled = true;
            previous.socks5_address = Some(proxy_address.to_string());
            previous.socks5_username = Some("previous-proxy-user-2c8f".to_string());
            previous.set_plugin_proxy_enabled(PROXY_PLUGIN_ID, true);
            config_service
                .save_user_config(&previous)
                .expect("persist previous proxy settings");
            dbs.secrets
                .set_secret("proxy:socks5", &previous_password)
                .expect("persist previous proxy password");

            let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
            let mut services = Services::new(runtime);
            services.core.config_service = Some(config_service.clone());
            services.async_http_client.apply_proxy_routing(
                Some(ProxyConfig {
                    enabled: previous.socks5_enabled,
                    address: previous.socks5_address.clone().unwrap(),
                    username: previous.socks5_username.clone(),
                    password: Some(previous_password.clone()),
                }),
                previous.get_plugin_proxy_settings(),
            );

            let signals = AppSignals::new();
            signals.user_config.set(previous.clone());
            let app_state = AppState {
                user_config: previous.clone(),
                pass_rules: vec![],
                backend_selector: BackendSelector::new_native(),
                fallback_backend: SevenZipCli::detect(None)
                    .expect("7z executable not found for proxy settings test"),
                last_entries: vec![],
                encrypted_crc_policy: "on_access".to_string(),
                db_paths: Some(paths),
                dbs: Some(dbs),
                plugin_event_scheduler: None,
                pending_plugin_events: Vec::new(),
                signals: signals.clone(),
            };
            let services = Arc::new(services);
            let plugin_ui_jobs = crate::features::plugins::application::PluginUiJobs::new(
                services.plugin_manager.clone(),
                services.tokio_runtime.clone(),
            );
            let image_assets = crate::shared::image_assets::ImageAssetStore::without_cache(
                services.tokio_runtime.clone(),
            );
            let shared = SharedState {
                app_state: Arc::new(Mutex::new(app_state)),
                services,
                theme: AppTheme::new(false),
                toaster: Arc::new(Mutex::new(Toaster::new())),
                refresh_requests: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                plugin_ui_jobs,
                image_assets,
                signals,
                facade: None,
            };

            Self {
                shared,
                config_service,
                previous,
                previous_password,
                proxy_listener,
                _temp: temp,
            }
        }

        fn assert_previous_settings_unchanged(&self) {
            self.assert_previous_non_secret_settings_unchanged();

            let state = self.shared.app_state.lock();
            let secret = state
                .dbs
                .as_ref()
                .unwrap()
                .secrets
                .get_secret("proxy:socks5")
                .expect("read proxy secret")
                .expect("previous proxy secret remains");
            assert_eq!(&*secret, self.previous_password.as_str());
        }

        fn assert_previous_non_secret_settings_unchanged(&self) {
            let state = self.shared.app_state.lock();
            assert_proxy_fields_eq(&state.user_config, &self.previous, "app state");
            drop(state);

            let signal = self.shared.signals.user_config.get();
            assert_proxy_fields_eq(&signal, &self.previous, "user-config signal");
            let persisted = self
                .config_service
                .get_user_config()
                .expect("reload persisted user config");
            assert_proxy_fields_eq(&persisted, &self.previous, "ConfigService persistence");
        }

        fn assert_previous_runtime_proxy_unchanged(&self) {
            let proxy = self
                .proxy_listener
                .try_clone()
                .expect("clone reserved proxy listener");
            proxy
                .set_nonblocking(true)
                .expect("make proxy sentinel nonblocking");
            let target = TcpListener::bind("127.0.0.1:0").expect("bind direct HTTP sentinel");
            target
                .set_nonblocking(true)
                .expect("make HTTP sentinel nonblocking");
            let target_address = target.local_addr().expect("read HTTP sentinel address");
            let reached_directly = Arc::new(AtomicBool::new(false));
            let reached_on_thread = reached_directly.clone();
            let reached_proxy = Arc::new(AtomicBool::new(false));
            let proxy_reached_on_thread = reached_proxy.clone();
            let request_finished = Arc::new(AtomicBool::new(false));
            let proxy_finished_on_thread = request_finished.clone();
            let proxy_server = std::thread::spawn(move || {
                serve_proxy_sentinel(proxy, proxy_finished_on_thread, proxy_reached_on_thread);
            });

            let target_finished_on_thread = request_finished.clone();
            let target_server = std::thread::spawn(move || {
                while !target_finished_on_thread.load(Ordering::SeqCst) {
                    match target.accept() {
                        Ok((mut socket, _)) => {
                            reached_on_thread.store(true, Ordering::SeqCst);
                            socket
                                .set_read_timeout(Some(Duration::from_secs(1)))
                                .unwrap();
                            let mut request = [0_u8; 1024];
                            let _ = socket.read(&mut request);
                            let _ = socket.write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                            );
                            return;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("HTTP sentinel accept failed: {error}"),
                    }
                }
            });

            let result = self.shared.services.async_http_client.blocking_get(
                &format!("http://{target_address}/runtime-proxy-sentinel"),
                true,
            );
            request_finished.store(true, Ordering::SeqCst);
            proxy_server.join().expect("proxy sentinel thread panicked");
            target_server.join().expect("HTTP sentinel thread panicked");

            assert!(result.is_err(), "invalid save made the request succeed");
            assert!(
                reached_proxy.load(Ordering::SeqCst),
                "invalid save replaced the previous runtime proxy configuration"
            );
            assert!(
                !reached_directly.load(Ordering::SeqCst),
                "invalid save replaced the previous runtime proxy with a direct client"
            );
        }
    }

    fn assert_proxy_fields_eq(actual: &UserConfig, expected: &UserConfig, surface: &str) {
        assert_eq!(
            actual.socks5_enabled, expected.socks5_enabled,
            "{surface} proxy enablement changed"
        );
        assert_eq!(
            actual.socks5_address, expected.socks5_address,
            "{surface} proxy address changed"
        );
        assert_eq!(
            actual.socks5_username, expected.socks5_username,
            "{surface} proxy username changed"
        );
    }

    fn assert_invalid_proxy_save_is_atomic(
        enabled: bool,
        address_user: &str,
        address_password: &str,
        direct_user: &str,
        direct_password: &str,
    ) {
        let fixture = ProxySaveFixture::new();
        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: enabled,
                socks5_address: Some(format!(
                    "{address_user}:{address_password}@proxy.example:1080"
                )),
                socks5_username: Some(direct_user.to_string()),
                socks5_password: Some(direct_password.to_string()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        fixture.assert_previous_settings_unchanged();
        fixture.assert_previous_runtime_proxy_unchanged();

        let observable_state = format!(
            "{:?} {:?} {:?}",
            fixture.shared.app_state.lock().user_config,
            fixture.shared.signals.user_config.get(),
            fixture.shared.toaster.lock()
        );
        for secret in [address_user, address_password, direct_user, direct_password] {
            assert!(
                !observable_state.contains(secret),
                "proxy secret reached state, persistence, or error surface: {observable_state}"
            );
        }
        assert!(observable_state.contains("<invalid address>"));
    }

    #[traced_test]
    #[test]
    fn saved_proxy_diagnostic_redacts_invalid_address_and_direct_credentials() {
        const ADDRESS_USER: &str = "ui-address-user-secret-0a7d";
        const ADDRESS_PASSWORD: &str = "ui-address-password-secret-4b2e";
        const DIRECT_USER: &str = "ui-direct-user-secret-8c1f";
        const DIRECT_PASSWORD: &str = "ui-direct-password-secret-6d3a";
        let config = ProxyConfig {
            enabled: true,
            address: format!("{ADDRESS_USER}:{ADDRESS_PASSWORD}@proxy.example:1080"),
            username: Some(DIRECT_USER.to_string()),
            password: Some(DIRECT_PASSWORD.to_string()),
        };

        log_saved_proxy_configuration(&config);

        for secret in [ADDRESS_USER, ADDRESS_PASSWORD, DIRECT_USER, DIRECT_PASSWORD] {
            assert!(!logs_contain(secret), "proxy secret leaked in tracing");
        }
        assert!(logs_contain("<invalid address>"));
        assert!(logs_contain("authenticated"));
    }

    #[traced_test]
    #[test]
    fn save_network_rejects_enabled_address_userinfo_before_any_mutation() {
        let secrets = [
            "enabled-address-user-secret-a14f",
            "enabled-address-password-secret-b25e",
            "enabled-direct-user-secret-c36d",
            "enabled-direct-password-secret-d47c",
        ];

        assert_invalid_proxy_save_is_atomic(true, secrets[0], secrets[1], secrets[2], secrets[3]);

        for secret in secrets {
            assert!(!logs_contain(secret), "proxy secret leaked in tracing");
        }
        assert!(logs_contain("<invalid address>"));
    }

    #[traced_test]
    #[test]
    fn save_network_rejects_disabled_nonempty_address_userinfo_before_any_mutation() {
        let secrets = [
            "disabled-address-user-secret-e58b",
            "disabled-address-password-secret-f69a",
            "disabled-direct-user-secret-a70c",
            "disabled-direct-password-secret-b81d",
        ];

        assert_invalid_proxy_save_is_atomic(false, secrets[0], secrets[1], secrets[2], secrets[3]);

        for secret in secrets {
            assert!(!logs_contain(secret), "proxy secret leaked in tracing");
        }
        assert!(logs_contain("<invalid address>"));
    }

    #[traced_test]
    #[test]
    fn save_network_accepts_blank_disabled_proxy_and_removes_password() {
        let fixture = ProxySaveFixture::new();

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: false,
                socks5_address: Some(String::new()),
                socks5_username: None,
                socks5_password: Some(String::new()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        let state = fixture.shared.app_state.lock();
        assert!(!state.user_config.socks5_enabled);
        assert_eq!(state.user_config.socks5_address.as_deref(), Some(""));
        assert!(state.user_config.socks5_username.is_none());
        let secret = state
            .dbs
            .as_ref()
            .unwrap()
            .secrets
            .get_secret("proxy:socks5")
            .unwrap();
        assert!(secret.is_none(), "blank password did not remove the secret");
        drop(state);

        let persisted = fixture.config_service.get_user_config().unwrap();
        assert!(!persisted.socks5_enabled);
        assert_eq!(persisted.socks5_address.as_deref(), Some(""));
        assert!(persisted.socks5_username.is_none());
        assert!(!format!("{:?}", fixture.shared.toaster.lock()).contains("Error"));
        assert!(!logs_contain("Refusing to save invalid proxy"));
        assert!(logs_contain("Network settings saved"));
    }

    #[test]
    fn save_network_disable_clears_runtime_plugin_proxy_map() {
        let fixture = ProxySaveFixture::new();
        assert!(fixture
            .shared
            .services
            .async_http_client
            .should_use_proxy_for_plugin(PROXY_PLUGIN_ID));

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: false,
                socks5_address: Some(String::new()),
                socks5_username: None,
                socks5_password: Some(String::new()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        assert!(
            !fixture
                .shared
                .services
                .async_http_client
                .should_use_proxy_for_plugin(PROXY_PLUGIN_ID),
            "disabling the global proxy left its per-plugin route enabled"
        );
    }

    #[test]
    fn save_network_reenable_restores_persisted_plugin_proxy_map() {
        let fixture = ProxySaveFixture::new();
        fixture
            .shared
            .services
            .async_http_client
            .apply_plugin_proxy_map(Default::default());
        assert!(!fixture
            .shared
            .services
            .async_http_client
            .should_use_proxy_for_plugin(PROXY_PLUGIN_ID));

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: true,
                socks5_address: Some("127.0.0.1:1080".to_string()),
                socks5_username: None,
                socks5_password: Some(String::new()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        assert!(
            fixture
                .shared
                .services
                .async_http_client
                .should_use_proxy_for_plugin(PROXY_PLUGIN_ID),
            "re-enabling the global proxy did not restore the persisted per-plugin route"
        );
    }

    #[test]
    fn save_network_reenable_matches_startup_proxy_defaults_and_explicit_overrides() {
        let fixture = ProxySaveFixture::new();
        {
            let mut state = fixture.shared.app_state.lock();
            state
                .user_config
                .set_plugin_proxy_enabled("dlsite-api", false);
        }

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: true,
                socks5_address: Some("127.0.0.1:1080".to_string()),
                socks5_username: None,
                socks5_password: Some(String::new()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        let client = &fixture.shared.services.async_http_client;
        assert!(
            client.should_use_proxy_for_plugin("dlsite"),
            "live re-enable omitted the startup default for dlsite"
        );
        assert!(
            !client.should_use_proxy_for_plugin("dlsite-api"),
            "live re-enable replaced an explicit dlsite-api opt-out"
        );
        assert!(
            client.should_use_proxy_for_plugin("dlsite-html"),
            "live re-enable omitted the startup default for dlsite-html"
        );
    }

    #[traced_test]
    #[test]
    fn config_persistence_failure_rolls_back_secret_and_does_not_apply() {
        const NEW_PASSWORD: &str = "new-proxy-password-4b65";
        let fixture = ProxySaveFixture::new();
        {
            let state = fixture.shared.app_state.lock();
            state
                .dbs
                .as_ref()
                .unwrap()
                .config
                .with_connection(|connection| {
                    connection.execute_batch(
                        "CREATE TRIGGER reject_proxy_config_save
                         BEFORE INSERT ON user_config
                         BEGIN
                             SELECT RAISE(ABORT, 'injected proxy config failure');
                         END;",
                    )?;
                    Ok(())
                })
                .expect("install failing config trigger");
        }

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: true,
                socks5_address: Some("127.0.0.1:1081".to_string()),
                socks5_username: Some("new-proxy-user-9c17".to_string()),
                socks5_password: Some(NEW_PASSWORD.to_string()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        fixture.assert_previous_settings_unchanged();
        fixture.assert_previous_runtime_proxy_unchanged();
        let diagnostics = format!("{:?}", fixture.shared.toaster.lock());
        assert!(diagnostics.contains("Network settings were not saved"));
        assert!(!diagnostics.contains(NEW_PASSWORD));
        assert!(!logs_contain(NEW_PASSWORD));
    }

    #[traced_test]
    #[test]
    fn unavailable_secrets_db_does_not_persist_or_apply_proxy() {
        const NEW_PASSWORD: &str = "unavailable-db-password-8f32";
        let fixture = ProxySaveFixture::new();
        fixture.shared.app_state.lock().dbs = None;

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: false,
                socks5_address: Some(String::new()),
                socks5_username: None,
                socks5_password: Some(NEW_PASSWORD.to_string()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        fixture.assert_previous_non_secret_settings_unchanged();
        fixture.assert_previous_runtime_proxy_unchanged();
        let diagnostics = format!("{:?}", fixture.shared.toaster.lock());
        assert!(diagnostics.contains("Network settings were not saved"));
        assert!(!diagnostics.contains(NEW_PASSWORD));
        assert!(!logs_contain(NEW_PASSWORD));
    }

    #[test]
    fn pending_proxy_marker_blocks_new_save_before_mutation() {
        let fixture = ProxySaveFixture::new();
        {
            let state = fixture.shared.app_state.lock();
            state
                .dbs
                .as_ref()
                .unwrap()
                .secrets
                .set_secret("journal:proxy-settings", "invalid-marker")
                .unwrap();
        }

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: true,
                socks5_address: Some("127.0.0.1:1081".to_string()),
                socks5_username: Some("new-user".to_string()),
                socks5_password: Some("new-password".to_string()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &fixture.shared,
        );

        fixture.assert_previous_settings_unchanged();
        fixture.assert_previous_runtime_proxy_unchanged();
        let state = fixture.shared.app_state.lock();
        assert_eq!(
            state
                .dbs
                .as_ref()
                .unwrap()
                .secrets
                .get_secret("journal:proxy-settings")
                .unwrap()
                .as_ref()
                .map(|value| value.as_str()),
            Some("invalid-marker")
        );
    }

    #[test]
    fn proxy_fixture_retains_exclusive_ownership_of_runtime_proxy_port() {
        let fixture = ProxySaveFixture::new();
        let proxy_address: std::net::SocketAddr = fixture
            .previous
            .socks5_address
            .as_deref()
            .unwrap()
            .parse()
            .unwrap();

        assert!(
            TcpListener::bind(proxy_address).is_err(),
            "fixture released its proxy port for reuse by a later sentinel"
        );
    }

    #[test]
    fn proxy_sentinel_remains_available_until_the_request_finishes() {
        let proxy = TcpListener::bind("127.0.0.1:0").expect("bind delayed proxy sentinel");
        let proxy_address = proxy.local_addr().expect("read delayed proxy address");
        proxy
            .set_nonblocking(true)
            .expect("make delayed proxy sentinel nonblocking");
        let request_finished = Arc::new(AtomicBool::new(false));
        let reached_proxy = Arc::new(AtomicBool::new(false));
        let server = {
            let request_finished = request_finished.clone();
            let reached_proxy = reached_proxy.clone();
            std::thread::spawn(move || {
                serve_proxy_sentinel(proxy, request_finished, reached_proxy);
            })
        };

        std::thread::sleep(Duration::from_millis(1_100));
        let _connection = std::net::TcpStream::connect(proxy_address)
            .expect("connect after the old scheduling deadline");
        server.join().expect("proxy sentinel thread panicked");
        request_finished.store(true, Ordering::SeqCst);

        assert!(
            reached_proxy.load(Ordering::SeqCst),
            "proxy sentinel stopped before the request lifecycle finished"
        );
    }
}
