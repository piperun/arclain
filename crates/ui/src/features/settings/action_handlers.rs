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

            if let Err(e) =
                state.apply_preferences(key_file_str, secrets_db_str, encrypted_crc_policy)
            {
                security_state.error = format!("Failed to save settings: {}", e);
            } else {
                security_state.info = "Settings saved successfully".to_string();
            }
        }
        SettingsAction::SaveArchives { temp_dir } => {
            let mut state = shared.app_state.lock();
            state.user_config.temp_dir = temp_dir;
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
            if let Err(e) = state.move_vault(&dest_path) {
                security_state.error = format!("Failed to move vault: {}", e);
            } else {
                security_state.info = "Vault moved successfully".to_string();
            }
        }
        SettingsAction::RekeyVault { new_key_file_path } => {
            let mut state = shared.app_state.lock();
            if let Err(e) = state.rekey_vault(&new_key_file_path) {
                security_state.error = format!("Failed to rekey vault: {}", e);
            } else {
                security_state.info = "Vault rekeyed successfully".to_string();
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
            let state = shared.app_state.lock();
            if let Some(manager) = &state.plugin_manager {
                let mut mgr = manager.lock();
                match mgr.install_plugin(std::path::Path::new(&wasm_path)) {
                    Ok(id) => {
                        tracing::info!("Successfully installed plugin: {}", id);
                        // Refresh list
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
                    archives_state.checksum_enabled = false;
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
        SettingsAction::NavigateTo(_) => {
            // Navigation is handled by extract_navigation before this function is called
        }
    }
}
