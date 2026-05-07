//! Unified Plugin Page
//!
//! Coordinator for plugin list and detail views.
//! Dispatches rendering to the appropriate view based on state.

use crate::features::plugins::domain::types::PluginsListState;
use crate::features::settings::domain::types::SettingsAction;

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_plugins::PluginManager;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

/// Render the Plugin Page (coordinator)
/// Dispatches to list_view or detail_view based on selection state
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    state: &mut PluginsListState,
    app_state: &Arc<Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
    content_cache: Option<Arc<arclain_core::ContentCache>>,
) -> Option<SettingsAction> {
    let action = None;

    if state.selected_plugin.is_some() {
        // Detail View
        let needs_refresh = crate::features::plugins::presentation::views::detail_view::render(
            ui,
            theme,
            plugin_manager,
            state,
            app_state,
            shared,
            content_cache.as_ref(),
        );

        if needs_refresh {
            if let Some(manager) = plugin_manager {
                let state_lock = app_state.lock();
                state.update_from_manager(manager, &state_lock.user_config);
            }
        }
    } else {
        // List View
        crate::features::plugins::presentation::views::list_view::render(ui, theme, state);
    }

    action
}

/// Generate header configuration for the Plugins page
pub fn get_header_config<'a>(
    state: &'a mut PluginsListState,
    page: &crate::core::SettingsPage,
    install_clicked_cell: &'a std::cell::Cell<bool>,
) -> crate::features::settings::presentation::views::header_config::SettingsHeaderConfig<'a> {
    use crate::features::settings::presentation::views::header_config::SettingsHeaderConfig;

    // Check if we are in Detail View
    if let Some(plugin_id) = state.selected_plugin.clone() {
        if let Some(plugin) = state.plugins.iter().find(|p| &p.id == &plugin_id) {
            let selected_plugin = &mut state.selected_plugin;

            let mut config = SettingsHeaderConfig::new(&plugin.name)
                .sub_description(format!(
                    "v{} by {}",
                    plugin.version,
                    plugin.author.as_deref().unwrap_or("Unknown")
                ))
                .has_changes(false) // Plugin settings save immediately, no Save button needed
                .on_back(|| {
                    *selected_plugin = None;
                });

            // Add actual plugin description if available
            if let Some(desc) = &plugin.description {
                config = config.description(desc.clone());
            }

            return config;
        }
    }

    // Default List View Header
    let filter_text = &mut state.filter_text;
    let show_permissions = &mut state.show_permissions;
    let show_disabled = &mut state.show_disabled;

    SettingsHeaderConfig::new(page.display_name())
        .icon(page.icon())
        .description(page.description())
        .secondary_row(move |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    crate::shared::components::SearchBar::new(filter_text)
                        .hint("Search plugins...")
                        .width(200.0),
                );
                ui.add_space(8.0);
                if ui
                    .add(arclain_widgets::TextButton::new(
                        "+ Install Plugin",
                        arclain_widgets::ButtonSize::Medium,
                    ))
                    .clicked()
                {
                    install_clicked_cell.set(true);
                }
            });
        })
        .tertiary_row(move |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(show_permissions, "Show Permission Tags");
                ui.add_space(16.0);
                ui.checkbox(show_disabled, "Show Disabled");
            });
        })
}
