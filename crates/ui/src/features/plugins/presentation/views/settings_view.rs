//! Plugins Settings Page
//!
//! Contains settings for managing installed plugins.

use crate::features::plugins::domain::types::PluginsListState;
use crate::features::settings::domain::types::SettingsAction;

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use eframe::egui;

/// Render the Plugins settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugins_state: &mut PluginsListState,
    shared: Option<&SharedState>,
) -> Option<SettingsAction> {
    if let Some(shared) = shared {
        crate::features::plugins::application::request_plugin_snapshot(shared, plugins_state);
    }

    // Render the plugin list
    // Render the unified plugin page
    // Note: plugins_page::render returns Option<SettingsAction>
    crate::features::plugins::presentation::pages::plugins_page::render(
        ui,
        theme,
        plugins_state,
        shared,
    )
}
