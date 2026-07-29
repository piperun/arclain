//! Settings action handlers
//!
//! Extracted from ui.rs to reduce file size and improve organization.

use crate::core::navigation::SettingsPage;
use crate::features::plugins::domain::types::PluginsListState;

use crate::features::settings::domain::types::{
    ArchivesSettingsState, SecuritySettingsState, ServerSettingsState, SettingsAction,
};
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
            let Some(facade) = shared.facade.as_ref() else {
                *security_state.error.write() = "Settings are unavailable right now".to_string();
                return;
            };
            let mut state = shared.app_state.lock();
            if let Err(e) = state.apply_preferences(
                facade,
                &shared.services.tokio_runtime,
                key_file_path,
                secrets_db_path,
                encrypted_crc_policy,
            ) {
                *security_state.error.write() = format!("Failed to save settings: {}", e);
            } else {
                *security_state.info.write() = "Settings saved successfully".to_string();
            }
        }
        SettingsAction::SaveArchives { temp_dir } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save archive settings: settings facade is unavailable");
                return;
            };
            let mut state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    general: None,
                    archive: Some(arclain_app::settings::ArchiveSettingsPatch {
                        backend_mode: arclain_app::settings::PatchValue::Keep,
                        cache_directory: arclain_app::settings::PatchValue::Keep,
                        temp_directory: match temp_dir {
                            Some(ref value) if !value.is_empty() => {
                                arclain_app::settings::PatchValue::Set(std::path::PathBuf::from(
                                    value,
                                ))
                            }
                            _ => arclain_app::settings::PatchValue::Clear,
                        },
                        transfer_directory: arclain_app::settings::PatchValue::Keep,
                        sevenzip_path: arclain_app::settings::PatchValue::Keep,
                    }),
                    network: None,
                    security: None,
                },
            );
            if let Err(e) = patch_result {
                tracing::error!("Failed to save user config: {}", e);
            }
        }
        SettingsAction::SaveKeyboardMouse { bindings } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save hotkey bindings: settings facade is unavailable");
                return;
            };
            let hotkey_bindings_json = serde_json::to_string(&bindings).ok();
            let mut state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    archive: None,
                    network: None,
                    security: None,
                    general: Some(arclain_app::settings::GeneralSettingsPatch {
                        hotkey_bindings: match hotkey_bindings_json {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                        open_nested_in_new_tab: arclain_app::settings::PatchValue::Keep,
                        drop_behavior: arclain_app::settings::PatchValue::Keep,
                        restore_tabs_on_launch: arclain_app::settings::PatchValue::Keep,
                    }),
                },
            );
            match patch_result {
                Ok(_) => tracing::info!("Hotkey bindings saved successfully"),
                Err(e) => tracing::error!("Failed to save hotkey bindings: {}", e),
            }

            // Signal app to reload hotkeys
            state.signals.hotkeys_updated.set(true);
        }
        SettingsAction::MoveVault { dest_path } => {
            let Some(facade) = shared.facade.as_ref() else {
                *security_state.error.write() = "Settings are unavailable right now".to_string();
                return;
            };
            let mut state = shared.app_state.lock();
            if let Err(e) = state.move_vault(facade, &shared.services.tokio_runtime, &dest_path) {
                *security_state.error.write() = format!("Failed to move vault: {}", e);
            } else {
                *security_state.info.write() = "Vault moved successfully".to_string();
            }
        }
        SettingsAction::RekeyVault { new_key_file_path } => {
            let Some(facade) = shared.facade.as_ref() else {
                *security_state.error.write() = "Settings are unavailable right now".to_string();
                return;
            };
            let mut state = shared.app_state.lock();
            if let Err(e) =
                state.rekey_vault(facade, &shared.services.tokio_runtime, &new_key_file_path)
            {
                *security_state.error.write() = format!("Failed to rekey vault: {}", e);
            } else {
                *security_state.info.write() = "Vault rekeyed successfully".to_string();
            }
        }
        SettingsAction::SavePasswordRules { rules } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save password rules: settings facade is unavailable");
                return;
            };
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
            if let Err(e) =
                state.save_password_rules(facade, &shared.services.tokio_runtime, core_rules)
            {
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
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("Cannot save general settings: settings facade is unavailable");
                return;
            };
            let mut state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    archive: None,
                    network: None,
                    security: None,
                    general: Some(arclain_app::settings::GeneralSettingsPatch {
                        hotkey_bindings: arclain_app::settings::PatchValue::Keep,
                        open_nested_in_new_tab: arclain_app::settings::PatchValue::Set(
                            open_nested_in_new_tab,
                        ),
                        drop_behavior: arclain_app::settings::PatchValue::Set(
                            drop_behavior.as_str().to_string(),
                        ),
                        restore_tabs_on_launch: arclain_app::settings::PatchValue::Set(
                            restore_tabs_on_launch,
                        ),
                    }),
                },
            );
            match patch_result {
                Ok(_) => tracing::info!("General settings saved"),
                Err(e) => tracing::error!("Failed to save general settings: {}", e),
            }
        }
        SettingsAction::SaveNetwork {
            socks5_enabled,
            socks5_address,
            socks5_username,
            socks5_password,
        } => {
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("[SaveNetwork] settings facade is unavailable");
                shared
                    .toaster
                    .lock()
                    .error("Network settings were not saved: application facade is unavailable");
                return;
            };

            // Validation (no embedded userinfo in the address, etc.) and
            // atomic persistence -- including preserving whatever SOCKS5
            // password is already stored -- now happen inside
            // `update_settings` itself (see `arclain_app::runtime::
            // settings_ops::validate_proxy_for_storage`); a rejected
            // patch here means nothing was written, matching this
            // handler's pre-facade "validate before any mutation"
            // guarantee.
            let mut state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    general: None,
                    archive: None,
                    network: Some(arclain_app::settings::NetworkSettingsPatch {
                        socks5_enabled: arclain_app::settings::PatchValue::Set(socks5_enabled),
                        socks5_address: match socks5_address.clone() {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                        socks5_username: match socks5_username.clone() {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                        plugin_proxy_enabled: arclain_app::settings::PatchValue::Keep,
                        gameta_server_enabled: arclain_app::settings::PatchValue::Keep,
                        gameta_server_url: arclain_app::settings::PatchValue::Keep,
                    }),
                    security: None,
                },
            );
            drop(state);

            let snapshot = match patch_result {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    // `error`'s message is already the short, safe-to-show
                    // `ApplicationError::summary` (see `submit_settings_patch`'s
                    // own doc comment) -- covers both an invalid address
                    // (rejected before any write) and a persistence
                    // failure with one consistent, specific message.
                    tracing::error!("[SaveNetwork] Failed to save network settings: {error}");
                    shared
                        .toaster
                        .lock()
                        .error(format!("Network settings were not saved: {error}"));
                    return;
                }
            };

            // The password is a dedicated secret write, deliberately
            // separate from the patch above (see `ArclainApp::
            // set_socks5_password`'s own doc comment). This page has no
            // "leave the password unchanged" affordance: a blank field
            // has always meant "no password" here.
            let password = socks5_password
                .filter(|password| !password.is_empty())
                .map(arclain_app::challenge::SecretInput::new);
            if let Err(error) = shared
                .services
                .tokio_runtime
                .block_on(facade.set_socks5_password(password))
            {
                tracing::error!("[SaveNetwork] Failed to save the SOCKS5 password: {error:?}");
                // Unlike the `patch_result` branch above, `snapshot` is
                // `Ok` here -- the address/identity patch already
                // committed before this call ever ran. "Network
                // settings were not saved" would be false in exactly
                // this branch, so this message says precisely what did
                // and didn't happen instead of reusing that wording.
                shared.toaster.lock().error(
                    "Network address and identity settings were saved, but the password \
                     change failed: the old password remains in effect",
                );
                return;
            }
            let _ = shared.app_state.lock().refresh_settings_from_facade(facade);

            // Live routing (`shared.services.async_http_client`, the
            // same `Arc` `update_settings` already applied it to -- see
            // `runtime::settings_ops::apply_live_proxy_routing`) is
            // already in effect by this point; only the log line is
            // this handler's own remaining job.
            let config = ProxyConfig {
                enabled: snapshot.network.socks5_enabled,
                address: snapshot.network.socks5_address.unwrap_or_default(),
                username: snapshot.network.socks5_username,
                password: None,
            };
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
            let Some(facade) = shared.facade.as_ref() else {
                tracing::error!("[SaveServer] settings facade is unavailable");
                return;
            };
            let mut state = shared.app_state.lock();
            let patch_result = state.submit_settings_patch(
                facade,
                &shared.services.tokio_runtime,
                |expected_revision| arclain_app::settings::SettingsPatch {
                    expected_revision,
                    general: None,
                    archive: None,
                    network: Some(arclain_app::settings::NetworkSettingsPatch {
                        socks5_enabled: arclain_app::settings::PatchValue::Keep,
                        socks5_address: arclain_app::settings::PatchValue::Keep,
                        socks5_username: arclain_app::settings::PatchValue::Keep,
                        plugin_proxy_enabled: arclain_app::settings::PatchValue::Keep,
                        gameta_server_enabled: arclain_app::settings::PatchValue::Set(enabled),
                        gameta_server_url: match url.clone() {
                            Some(value) => arclain_app::settings::PatchValue::Set(value),
                            None => arclain_app::settings::PatchValue::Clear,
                        },
                    }),
                    security: None,
                },
            );
            drop(state);
            match patch_result {
                Ok(_) => tracing::info!(
                    "[SaveServer] Server settings saved: enabled={}, url={:?}",
                    enabled,
                    url
                ),
                Err(error) => {
                    tracing::error!("[SaveServer] Failed to save server settings: {error}")
                }
            }

            // The API key is a dedicated secret write, kept separate
            // from the patch above -- see `ArclainApp::
            // set_gameta_api_key`'s own doc comment on why it has no
            // "clear" affordance (unlike the SOCKS5 password).
            if let Some(key) = api_key {
                let result = shared.services.tokio_runtime.block_on(
                    facade.set_gameta_api_key(arclain_app::challenge::SecretInput::new(key)),
                );
                match result {
                    Ok(()) => {
                        let _ = shared.app_state.lock().refresh_settings_from_facade(facade);
                    }
                    Err(error) => {
                        tracing::error!("[SaveServer] Failed to save API key: {error:?}");
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
    use arclain_app::ArclainApp;
    use arclain_core::services::ConfigService;
    use arclain_core::UserConfig;
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

    #[cfg(windows)]
    fn sevenzip_exe_name() -> &'static str {
        "7zz.exe"
    }

    #[cfg(not(windows))]
    fn sevenzip_exe_name() -> &'static str {
        "7zz"
    }

    /// Bootstraps a real `ArclainApp` for settings-page tests that
    /// exercise the actual facade-backed save path -- `SaveNetwork`/
    /// `SaveServer`/`SaveSecurity`/etc. now build a patch and submit it
    /// through `shared.facade`, so a fixture with `facade: None` (this
    /// module's own pre-facade convention, still used by
    /// `crates/ui/tests/common/mod.rs` for unrelated dispatcher tests)
    /// would never exercise them at all. Seeds a dummy 7-Zip the same
    /// way `arclain_app`'s own `tests/support::seed_working_sevenzip_config`
    /// does -- that module is test-private to the `arclain_app` package
    /// and unreachable from here, so this reimplements just the one seam
    /// these tests need.
    fn bootstrap_test_facade(temp: &TempDir) -> ArclainApp {
        let paths = arclain_app::AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };

        let sevenzip_path = temp.path().join(sevenzip_exe_name());
        std::fs::write(
            &sevenzip_path,
            b"not a real binary, only its path is checked",
        )
        .expect("write dummy 7-Zip executable");

        let databases_dir = paths.data_dir.join("databases");
        std::fs::create_dir_all(&databases_dir).expect("create databases dir");
        let config_db_path = databases_dir.join("config.sqlite");
        let db = arclain_core::config::ConfigDb::open(&config_db_path).expect("open config db");
        let conn = db.into_sqlite_db();
        conn.with_connection(|conn| {
            UserConfig::ensure_table(conn)?;
            let mut config = UserConfig::new();
            config.sevenzip_path = Some(sevenzip_path.to_string_lossy().into_owned());
            config.save(conn)?;
            Ok(())
        })
        .expect("seed sevenzip_path into test config db");

        arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
            paths_override: Some(paths),
            worker_threads: None,
            archive_backend_override: None,
            extract_runner_override: None,
            materialization_lease_ttl_override: None,
            materialization_cleanup_interval_override: None,
        })
        .expect("bootstrap the settings-page test facade")
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
            let facade = bootstrap_test_facade(&temp);

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

            // Seed the "previous" settings through the facade itself
            // (the same public surface a real prior session would have
            // used), not by reaching into its databases directly.
            let mut plugin_proxy_map = std::collections::BTreeMap::new();
            plugin_proxy_map.insert(PROXY_PLUGIN_ID.to_string(), true);
            let seed_runtime = tokio::runtime::Runtime::new().expect("create seeding runtime");
            seed_runtime.block_on(async {
                let current = facade.settings().await.expect("read first-run settings");
                facade
                    .update_settings(arclain_app::settings::SettingsPatch {
                        expected_revision: current.revision,
                        general: None,
                        archive: None,
                        network: Some(arclain_app::settings::NetworkSettingsPatch {
                            socks5_enabled: arclain_app::settings::PatchValue::Set(true),
                            socks5_address: arclain_app::settings::PatchValue::Set(
                                proxy_address.to_string(),
                            ),
                            socks5_username: arclain_app::settings::PatchValue::Set(
                                "previous-proxy-user-2c8f".to_string(),
                            ),
                            plugin_proxy_enabled: arclain_app::settings::PatchValue::Set(
                                plugin_proxy_map,
                            ),
                            gameta_server_enabled: arclain_app::settings::PatchValue::Keep,
                            gameta_server_url: arclain_app::settings::PatchValue::Keep,
                        }),
                        security: None,
                    })
                    .await
                    .expect("persist previous proxy settings");
                facade
                    .set_socks5_password(Some(arclain_app::challenge::SecretInput::new(
                        previous_password.clone(),
                    )))
                    .await
                    .expect("persist previous proxy password");
            });

            let legacy = facade
                .take_legacy_composition()
                .expect("take legacy composition for the test fixture");
            let config_service = legacy
                .core_services
                .config_service
                .clone()
                .expect("config service must be available after a real bootstrap");

            let services = Services {
                core: (*legacy.core_services).clone(),
                plugin_manager: legacy.plugin_manager,
                content_cache: legacy.content_cache,
                resource_manager: legacy.resource_manager,
            };
            let services = Arc::new(services);

            let signals = AppSignals::new();
            signals.user_config.set(legacy.user_config.clone());
            let app_state = AppState {
                user_config: legacy.user_config,
                pass_rules: legacy.pass_rules,
                backend_selector: legacy.backend_selector,
                fallback_backend: legacy.fallback_backend,
                last_entries: vec![],
                encrypted_crc_policy: legacy.encrypted_crc_policy,
                db_paths: legacy.db_paths,
                dbs: legacy.dbs,
                signals: signals.clone(),
            };

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
                facade: Some(facade),
                operation_origins: crate::core::operation_bridge::OperationOrigins::new(),
                materialization_actions: crate::core::operation_bridge::MaterializationActions::new(
                ),
                external_open_leases: crate::core::operation_bridge::ExternalOpenLeases::new(),
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
        // Seed the explicit per-plugin opt-out through the facade
        // itself -- it, not `AppState.user_config`, is what
        // `SaveNetwork` now reads its starting point from (see
        // `submit_settings_patch`), so mutating the legacy mirror
        // directly would no longer have any effect on the save this
        // test is about to trigger.
        let facade = fixture
            .shared
            .facade
            .as_ref()
            .expect("fixture must have a real facade");
        fixture.shared.services.tokio_runtime.block_on(async {
            let current = facade.settings().await.expect("read current settings");
            let mut plugin_proxy_enabled = current.network.plugin_proxy_enabled.clone();
            plugin_proxy_enabled.insert("dlsite-api".to_string(), false);
            facade
                .update_settings(arclain_app::settings::SettingsPatch {
                    expected_revision: current.revision,
                    general: None,
                    archive: None,
                    network: Some(arclain_app::settings::NetworkSettingsPatch {
                        socks5_enabled: arclain_app::settings::PatchValue::Keep,
                        socks5_address: arclain_app::settings::PatchValue::Keep,
                        socks5_username: arclain_app::settings::PatchValue::Keep,
                        plugin_proxy_enabled: arclain_app::settings::PatchValue::Set(
                            plugin_proxy_enabled,
                        ),
                        gameta_server_enabled: arclain_app::settings::PatchValue::Keep,
                        gameta_server_url: arclain_app::settings::PatchValue::Keep,
                    }),
                    security: None,
                })
                .await
                .expect("seed the dlsite-api opt-out");
        });

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

    /// `SaveNetwork` now submits two separate facade calls: `update_settings`
    /// for the identity fields (enabled/address/username), then
    /// `set_socks5_password` for the password -- both through
    /// `NetworkProxyPersistenceService::save`'s journaled path, but as two
    /// *separate* calls, not the pre-facade handler's one-shot combined
    /// save. A `user_config`-row failure trips `update_settings`'s own
    /// identity step *first* (it always runs before the password step),
    /// so this proves `SaveNetwork`'s atomicity from the UI's point of
    /// view: when the identity step fails, `set_socks5_password` is never
    /// even attempted (the handler returns early -- see `settings_
    /// controller.rs`'s own `SaveNetwork` arm), so neither the identity
    /// fields nor the password change apply, and the new password never
    /// reaches the toaster or logs.
    ///
    /// This does not exercise `set_socks5_password`'s *own* journaled
    /// rollback in isolation -- for a password-only change, the identity
    /// snapshot `NetworkProxyPersistenceService::save` tracks is
    /// unchanged (`previous == candidate`), which its own ambiguity
    /// resolution always treats as committed regardless of the
    /// underlying config-row write's outcome, so there is no config-row
    /// failure mode to roll a password-only change back from. That
    /// mechanism (a stale/corrupt pending marker failing the call
    /// cleanly rather than being silently ignored) is covered precisely
    /// at the layer that owns it:
    /// `arclain_app::tests::settings_facade::
    /// set_socks5_password_fails_cleanly_instead_of_ignoring_a_corrupt_pending_marker`.
    #[traced_test]
    #[test]
    fn config_persistence_failure_during_the_identity_step_blocks_the_whole_save() {
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
        // Reproduces "the vault genuinely never opened" by corrupting
        // `config.sqlite` before bootstrap -- the same technique
        // `arclain_app`'s own `tests/bootstrap.rs::
        // corrupt_configuration_database_is_tolerated` test uses
        // (`open_databases` composes config+secrets+metadata as one
        // atomic unit; a failure in any one part leaves `dbs: None`
        // overall -- see `arclain_app::tests::settings_facade::
        // secret_writing_methods_fail_cleanly_and_leak_nothing_when_the_
        // vault_never_opened` for the equivalent facade-level coverage
        // this mirrors at the UI layer). Unlike `ProxySaveFixture`, this
        // cannot seed any "previous" proxy settings first -- there is no
        // open vault to write them into -- so the meaningful assertion
        // here is "still at first-run defaults, nothing leaked", not
        // "the prior custom config survived".
        const NEW_PASSWORD: &str = "unavailable-db-password-8f32";
        let temp = tempfile::tempdir().expect("create test directory");
        let paths = arclain_app::AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        let databases_dir = paths.data_dir.join("databases");
        std::fs::create_dir_all(&databases_dir).expect("create databases dir");
        std::fs::write(
            databases_dir.join("config.sqlite"),
            b"this is not a sqlite database, just noise to corrupt the file",
        )
        .expect("write corrupt config.sqlite");
        // No dummy 7-Zip seeded: the corrupt file wipes any seeded
        // `sevenzip_path` override, so this relies on this project's
        // documented test-environment assumption that 7-Zip is on
        // `PATH` (see `arclain_app`'s own `tests/bootstrap.rs` module
        // doc comment).
        let facade = arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
            paths_override: Some(paths),
            worker_threads: None,
            archive_backend_override: None,
            extract_runner_override: None,
            materialization_lease_ttl_override: None,
            materialization_cleanup_interval_override: None,
        })
        .expect("corrupt config.sqlite must not fail bootstrap");

        let legacy = facade
            .take_legacy_composition()
            .expect("take legacy composition");
        assert!(
            legacy.dbs.is_none(),
            "test setup must reproduce a genuinely unavailable vault"
        );

        let services = Services {
            core: (*legacy.core_services).clone(),
            plugin_manager: legacy.plugin_manager,
            content_cache: legacy.content_cache,
            resource_manager: legacy.resource_manager,
        };
        let services = Arc::new(services);
        let signals = AppSignals::new();
        signals.user_config.set(legacy.user_config.clone());
        let app_state = AppState {
            user_config: legacy.user_config,
            pass_rules: legacy.pass_rules,
            backend_selector: legacy.backend_selector,
            fallback_backend: legacy.fallback_backend,
            last_entries: vec![],
            encrypted_crc_policy: legacy.encrypted_crc_policy,
            db_paths: legacy.db_paths,
            dbs: legacy.dbs,
            signals: signals.clone(),
        };
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
            facade: Some(facade),
            operation_origins: crate::core::operation_bridge::OperationOrigins::new(),
            materialization_actions: crate::core::operation_bridge::MaterializationActions::new(),
            external_open_leases: crate::core::operation_bridge::ExternalOpenLeases::new(),
        };

        handle_action(
            SettingsAction::SaveNetwork {
                socks5_enabled: true,
                socks5_address: Some("127.0.0.1:1080".to_string()),
                socks5_username: None,
                socks5_password: Some(NEW_PASSWORD.to_string()),
            },
            &mut SecuritySettingsState::default(),
            &mut ArchivesSettingsState::default(),
            None,
            &mut crate::features::settings::domain::types::NetworkSettingsState::default(),
            &mut ServerSettingsState::default(),
            &shared,
        );

        // Nothing persisted: still at first-run defaults.
        let state = shared.app_state.lock();
        assert!(
            !state.user_config.socks5_enabled,
            "no vault available -- the save must not have applied"
        );
        drop(state);

        let diagnostics = format!("{:?}", shared.toaster.lock());
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
