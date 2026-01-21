//! Plugins Settings Page
//!
//! Contains settings for managing installed plugins.

use crate::features::plugins::domain::types::PluginsListState;
use crate::features::settings::domain::types::SettingsAction;

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_plugins::PluginManager;
use eframe::egui;

/// Render the Plugins settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    plugins_state: &mut PluginsListState,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
) -> Option<SettingsAction> {
    // Update plugin list from manager if available
    if let Some(manager) = plugin_manager {
        let state = app_state.lock();
        plugins_state.update_from_manager(manager, &state.user_config);
    }

    // Extract content_cache from services (no lock needed)
    let content_cache = shared.and_then(|s| s.services.content_cache.clone());

    // Render the plugin list
    // Render the unified plugin page
    // Note: plugins_page::render returns Option<SettingsAction>
    crate::features::plugins::presentation::pages::plugins_page::render(
        ui,
        theme,
        plugin_manager,
        plugins_state,
        app_state,
        shared,
        content_cache,
    )
}
