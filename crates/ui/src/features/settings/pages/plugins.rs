//! Plugins Settings Page
//!
//! Contains settings for managing installed plugins.

use crate::features::plugins::plugin_list;
use crate::features::plugins::types::PluginsListState;
use crate::features::settings::types::SettingsAction;
use crate::shared::theme::AppTheme;
use arclain_plugins::PluginManager;
use eframe::egui;

/// Render the Plugins settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    plugins_state: &mut PluginsListState,
) -> Option<SettingsAction> {
    // Update plugin list from manager if available
    if let Some(manager) = plugin_manager {
        plugins_state.update_from_manager(manager);
    }

    // Render the plugin list
    if let Some(action) = plugin_list::render(ui, theme, plugins_state) {
        // Handle plugin actions
        return match action {
            plugin_list::PluginAction::SelectPlugin(id) => {
                plugins_state.selected_plugin = Some(id);
                None
            }
            plugin_list::PluginAction::EnablePlugin(id) => {
                if let Some(manager) = plugin_manager {
                    match manager.enable_plugin(&id) {
                        Ok(()) => {
                            tracing::info!("Plugin enabled: {}", id);
                            // Update the state immediately
                            plugins_state.update_from_manager(manager);
                        }
                        Err(e) => {
                            tracing::error!("Failed to enable plugin {}: {}", id, e);
                        }
                    }
                }
                None
            }
            plugin_list::PluginAction::DisablePlugin(id) => {
                if let Some(manager) = plugin_manager {
                    match manager.disable_plugin(&id) {
                        Ok(()) => {
                            tracing::info!("Plugin disabled: {}", id);
                            // Update the state immediately
                            plugins_state.update_from_manager(manager);
                        }
                        Err(e) => {
                            tracing::error!("Failed to disable plugin {}: {}", id, e);
                        }
                    }
                }
                None
            }
            plugin_list::PluginAction::ShowPluginSettings(id) => {
                // TODO: Implement settings dialog
                tracing::info!("Show settings for plugin: {}", id);
                None
            }
            plugin_list::PluginAction::InstallPlugin => {
                // Show file picker for .wasm files
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("WASM Plugin", &["wasm"])
                    .set_title("Select Plugin to Install")
                    .pick_file()
                {
                    tracing::info!("Selected plugin file: {}", file.display());
                    // Return action to be handled at app level where we have mutable access
                    Some(SettingsAction::InstallPlugin {
                        wasm_path: file.to_string_lossy().to_string(),
                    })
                } else {
                    None
                }
            }
        };
    }

    None
}
