//! Settings Content
//!
//! Re-exports types and page rendering functions for settings.

use crate::core::SettingsPage;
use crate::features::password_management::dialogs::zip_pass_rules::PasswordRulesDialog;
use crate::features::password_management::rules_page as password_rules_page;
use crate::features::plugins::domain::types::PluginsListState;

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_plugins::PluginManager;
use eframe::egui;

// Re-export types for backwards compatibility
pub use crate::features::settings::domain::types::*;

// Re-export page render functions
use crate::features::settings::presentation::pages;

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
    shared: Option<&SharedState>,
) -> Option<SettingsAction> {
    pages::plugins::render(ui, theme, plugin_manager, plugins_state, app_state, shared)
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
    rules_page: Option<&mut crate::features::settings::presentation::pages::RulesPage>,
    profiles_page: Option<&mut crate::features::settings::presentation::pages::ProfilesPage>,

    interface_state: &mut crate::features::settings::presentation::pages::interface::InterfaceSettingsState,
    toolbar_layout_state: &mut crate::features::settings::presentation::pages::ToolbarLayoutState,
    info_panel_layout_state: &mut crate::features::settings::presentation::pages::InfoPanelLayoutState,
    keyboard_mouse_state: &mut crate::features::settings::presentation::pages::keyboard_mouse::KeyboardMouseSettingsState,

    network_state: &mut NetworkSettingsState,
    server_state: &mut ServerSettingsState,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
) -> Option<SettingsAction> {
    match page {
        SettingsPage::Overview => {
            // This shouldn't be called as overview has its own rendering
            None
        }
        SettingsPage::General => render_general_settings(ui, theme, general_state),
        SettingsPage::Network => pages::network::render(ui, theme, network_state),
        SettingsPage::Server => pages::server::render(ui, theme, server_state),
        SettingsPage::Interface => {
            if let Some(shared_state) = shared {
                let ui_service = shared_state.services.ui_service.as_deref();
                crate::features::settings::presentation::pages::render_interface_settings(
                    ui,
                    theme,
                    shared_state,
                    interface_state,
                    ui_service,
                )
            } else {
                None
            }
        }
        SettingsPage::Archives => render_archives_settings(ui, theme, archives_state),
        SettingsPage::Security => render_security_settings(ui, theme, security_state),
        SettingsPage::PasswordRules => {
            render_password_rules_settings(ui, theme, password_rules_dialog)
        }
        SettingsPage::OrganizationRules => {
            if let Some(rp) = rules_page {
                if let Some(shared_state) = shared {
                    if let Some(org_service) = shared_state.services.organization_service.as_ref() {
                        return rp.render(ui, theme, org_service);
                    } else {
                        ui.label("Organization service not available.");
                    }
                } else {
                    ui.label("SharedState not available.");
                }
            } else {
                ui.label("Rules page not available.");
            }
            None
        }
        SettingsPage::Plugins => {
            render_plugins_settings(ui, theme, plugin_manager, plugins_state, app_state, shared)
        }
        SettingsPage::ToolbarLayout => {
            let ui_service = shared.and_then(|s| s.services.ui_service.as_deref());
            crate::features::settings::presentation::pages::render_toolbar_layout(
                ui,
                theme,
                ui_service,
                toolbar_layout_state,
                plugin_manager,
            );
            None
        }
        SettingsPage::InfoPanelLayout => {
            let ui_service = shared.and_then(|s| s.services.ui_service.as_deref());
            crate::features::settings::presentation::pages::render_info_panel_layout(
                ui,
                theme,
                ui_service,
                info_panel_layout_state,
                plugin_manager,
            );
            None
        }
        SettingsPage::KeyboardMouse => {
            pages::keyboard_mouse::render(ui, theme, keyboard_mouse_state)
        }
        SettingsPage::ArchiveProfiles => {
            if let Some(pp) = profiles_page {
                if let Some(shared_state) = shared {
                    pp.render(ui, theme, shared_state);
                }
            }
            None
        }
        SettingsPage::EditRule(rule_id) => {
            if let Some(rp) = rules_page {
                if let Some(shared_state) = shared {
                    if let Some(org_service) = shared_state.services.organization_service.as_ref() {
                        if let Some(editor_action) = rp.render_edit_rule(
                            ui,
                            theme,
                            org_service,
                            *rule_id,
                            plugin_manager,
                        ) {
                            use crate::features::settings::presentation::pages::RuleEditorAction;
                            match editor_action {
                                RuleEditorAction::Saved | RuleEditorAction::Cancelled => {
                                    // Navigate back to organization rules list
                                    return Some(SettingsAction::NavigateTo(SettingsPage::OrganizationRules));
                                }
                                RuleEditorAction::None => {}
                            }
                        }
                    }
                }
            }
            None
        }
    }
}
