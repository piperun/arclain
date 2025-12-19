//! Settings Content
//!
//! Re-exports types and page rendering functions for settings.

use crate::core::SettingsPage;
use crate::features::password_management::dialogs::zip_pass_rules::PasswordRulesDialog;
use crate::features::password_management::rules_page as password_rules_page;
use crate::features::plugins::types::PluginsListState;
use crate::shared::theme::AppTheme;
use arclain_plugins::PluginManager;
use eframe::egui;

// Re-export types for backwards compatibility
pub use crate::features::settings::types::*;

// Re-export page render functions
use crate::features::settings::pages;

/// Render the General settings page
pub fn render_general_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut GeneralSettingsState,
) -> Option<SettingsAction> {
    pages::general::render(ui, theme, state)
}

/// Render the Archives settings page
pub fn render_archives_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ArchivesSettingsState,
) -> Option<SettingsAction> {
    pages::archives::render(ui, theme, state)
}

/// Render the Security settings page
pub fn render_security_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut SecuritySettingsState,
) -> Option<SettingsAction> {
    pages::security::render(ui, theme, state)
}

/// Render the Password Rules settings page
/// Returns SavePasswordRules action if save button was clicked
pub fn render_password_rules_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    password_rules_dialog: &mut PasswordRulesDialog,
) -> Option<SettingsAction> {
    // Render the full password rules management page directly
    password_rules_page::render_password_rules_page(ui, theme, password_rules_dialog);
    None
}

/// Render the Plugins settings page
pub fn render_plugins_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    plugins_state: &mut PluginsListState,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
) -> Option<SettingsAction> {
    pages::plugins::render(ui, theme, plugin_manager, plugins_state, app_state)
}

/// Render the appropriate settings content based on the current page
pub fn render_settings_content(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    page: &SettingsPage,
    general_state: &mut GeneralSettingsState,
    security_state: &mut SecuritySettingsState,
    archives_state: &mut ArchivesSettingsState,
    password_rules_dialog: &mut PasswordRulesDialog,
    plugin_manager: Option<&PluginManager>,
    plugins_state: &mut PluginsListState,
    rules_page: Option<&mut crate::features::settings::pages::RulesPage>,
    interface_state: &mut crate::features::settings::pages::interface::InterfaceSettingsState,
    toolbar_layout_state: &mut crate::features::settings::pages::ToolbarLayoutState,
    info_panel_layout_state: &mut crate::features::settings::pages::InfoPanelLayoutState,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
) -> Option<SettingsAction> {
    match page {
        SettingsPage::Overview => {
            // This shouldn't be called as overview has its own rendering
            None
        }
        SettingsPage::General => render_general_settings(ui, theme, general_state),
        SettingsPage::Interface => crate::features::settings::pages::render_interface_settings(
            ui,
            theme,
            app_state,
            interface_state,
        ),
        SettingsPage::Archives => render_archives_settings(ui, theme, archives_state),
        SettingsPage::Security => render_security_settings(ui, theme, security_state),
        SettingsPage::PasswordRules => {
            render_password_rules_settings(ui, theme, password_rules_dialog)
        }
        SettingsPage::OrganizationRules => {
            if let Some(rp) = rules_page {
                let db_opt = {
                    let state = app_state.lock();
                    if let Some(dbs) = &state.dbs {
                        Some(dbs.config.clone())
                    } else {
                        None
                    }
                };

                if let Some(db) = db_opt {
                    rp.render(ui, theme, &db);
                } else {
                    ui.label("Database not available (encrypted?)");
                }
            } else {
                ui.label("Rules page not available.");
            }
            None
        }
        SettingsPage::Plugins => {
            render_plugins_settings(ui, theme, plugin_manager, plugins_state, app_state)
        }
        SettingsPage::ToolbarLayout => {
            crate::features::settings::pages::render_toolbar_layout(
                ui,
                theme,
                app_state,
                toolbar_layout_state,
            );
            None
        }
        SettingsPage::InfoPanelLayout => {
            crate::features::settings::pages::render_info_panel_layout(
                ui,
                theme,
                app_state,
                info_panel_layout_state,
            );
            None
        }
    }
}
