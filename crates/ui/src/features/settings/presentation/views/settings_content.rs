//! Settings Content
//!
//! Re-exports types and page rendering functions for settings.

use crate::core::SettingsPage;
use crate::features::password_management::dialogs::zip_pass_rules::PasswordRulesDialog;
use crate::features::password_management::rules_page as password_rules_page;
use crate::features::plugins::domain::types::PluginsListState;

use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
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
    config_service: Option<&arclain_core::services::ConfigService>,
) -> Option<SettingsAction> {
    pages::archives::render(ui, theme, state, config_service)
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
    plugins_state: &mut PluginsListState,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
) -> Option<SettingsAction> {
    crate::features::plugins::presentation::views::render_plugins_settings(
        ui,
        theme,
        plugins_state,
        app_state,
        shared,
    )
}

/// Bundle of borrowed state passed into [`render_settings_content`].
///
/// Each field is borrowed from its owning location: settings-owned page
/// state from [`SettingsFeature`](crate::features::settings::presentation::feature::SettingsFeature)
/// itself, and cross-feature state from sibling features via
/// [`SettingsFeatureBorrows`](crate::features::settings::presentation::feature::SettingsFeatureBorrows).
///
/// Bundling these 13 references into one struct keeps
/// `render_settings_content` from drowning in positional args
/// (`clippy::too_many_arguments` was firing at 19/7 pre-bundle).
pub struct SettingsContentBorrows<'a> {
    // Settings-owned page states
    pub general: &'a mut GeneralSettingsState,
    pub security: &'a mut SecuritySettingsState,
    pub archives: &'a mut ArchivesSettingsState,
    pub network: &'a mut NetworkSettingsState,
    pub server: &'a mut ServerSettingsState,
    pub interface:
        &'a mut crate::features::settings::presentation::pages::interface::InterfaceSettingsState,
    pub toolbar_layout: &'a mut crate::features::settings::presentation::pages::ToolbarLayoutState,
    pub info_panel_layout:
        &'a mut crate::features::settings::presentation::pages::InfoPanelLayoutState,

    // Cross-feature borrowed state (Optional because they may not exist)
    pub password_rules_dialog: Option<&'a mut PasswordRulesDialog>,
    pub plugins_state: Option<&'a mut PluginsListState>,
    pub keyboard_mouse_state:
        Option<&'a mut crate::features::hotkeys::presentation::KeyboardMouseSettingsState>,
    pub rules_page: Option<&'a mut crate::features::organization::presentation::views::RulesPage>,
    pub profiles_page:
        Option<&'a mut crate::features::organization::presentation::views::ProfilesPage>,
}

/// Render the appropriate settings content based on the current page
pub fn render_settings_content(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    page: &SettingsPage,
    borrows: SettingsContentBorrows<'_>,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    shared: Option<&SharedState>,
) -> Option<SettingsAction> {
    let SettingsContentBorrows {
        general: general_state,
        security: security_state,
        archives: archives_state,
        network: network_state,
        server: server_state,
        interface: interface_state,
        toolbar_layout: toolbar_layout_state,
        info_panel_layout: info_panel_layout_state,
        password_rules_dialog,
        plugins_state,
        keyboard_mouse_state,
        rules_page,
        profiles_page,
    } = borrows;

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
                if let Some(action) =
                    crate::features::settings::presentation::pages::render_interface_settings(
                        ui,
                        theme,
                        shared_state,
                        interface_state,
                    )
                {
                    use crate::features::settings::presentation::pages::InterfaceSettingsAction;
                    match action {
                        InterfaceSettingsAction::Navigate(page) => {
                            return Some(SettingsAction::NavigateTo(page));
                        }
                        other => {
                            crate::features::settings::presentation::pages::handle_interface_settings_action(
                                interface_state,
                                other,
                                shared_state,
                            );
                        }
                    }
                }
                None
            } else {
                None
            }
        }
        SettingsPage::Archives => {
            let cfg = shared.and_then(|s| s.services.config_service.as_deref());
            render_archives_settings(ui, theme, archives_state, cfg)
        }
        SettingsPage::Security => render_security_settings(ui, theme, security_state),
        SettingsPage::PasswordRules => {
            if let Some(dialog) = password_rules_dialog {
                render_password_rules_settings(ui, theme, dialog)
            } else {
                ui.label("Password management feature not available.");
                None
            }
        }
        SettingsPage::OrganizationRules => {
            if let Some(rp) = rules_page {
                if let Some(shared_state) = shared {
                    if let Some(org_service) = shared_state.services.organization_service.as_ref() {
                        if let Some(action) = rp.render(ui, theme) {
                            use crate::features::organization::presentation::views::RulesPageAction;
                            match action {
                                RulesPageAction::Navigate(page) => {
                                    return Some(SettingsAction::NavigateTo(page));
                                }
                                other => {
                                    let user_config = shared_state.signals().user_config.get();
                                    let plugins =
                                        shared_state.plugin_ui_jobs.plugin_snapshot(&user_config);
                                    crate::features::organization::presentation::views::rules_page::handle_rules_page_action(
                                        rp,
                                        other,
                                        org_service,
                                        plugins.as_deref().map(Vec::as_slice),
                                    );
                                }
                            }
                        }
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
            if let Some(plugins_state) = plugins_state {
                render_plugins_settings(ui, theme, plugins_state, app_state, shared)
            } else {
                ui.label("Plugins feature not available.");
                None
            }
        }
        SettingsPage::ToolbarLayout => {
            if let Some(action) =
                crate::features::settings::presentation::pages::render_toolbar_layout(
                    ui,
                    theme,
                    toolbar_layout_state,
                )
            {
                if let Some(shared_state) = shared {
                    crate::features::settings::presentation::pages::handle_toolbar_layout_action(
                        toolbar_layout_state,
                        action,
                        shared_state,
                    );
                }
            }
            None
        }
        SettingsPage::InfoPanelLayout => {
            if let Some(action) =
                crate::features::settings::presentation::pages::render_info_panel_layout(
                    ui,
                    theme,
                    info_panel_layout_state,
                )
            {
                if let Some(shared_state) = shared {
                    crate::features::settings::presentation::pages::handle_info_panel_layout_action(
                        info_panel_layout_state,
                        action,
                        shared_state,
                    );
                }
            }
            None
        }
        SettingsPage::KeyboardMouse => {
            if let Some(s) = keyboard_mouse_state {
                crate::features::hotkeys::presentation::render_keyboard_mouse(ui, theme, s)
            } else {
                ui.label("Hotkeys feature not available.");
                None
            }
        }
        SettingsPage::ArchiveProfiles => {
            if let Some(pp) = profiles_page {
                if let Some(shared_state) = shared {
                    if let Some(action) = pp.render(ui, theme) {
                        crate::features::organization::presentation::views::profiles_page::handle_profiles_action(
                            pp,
                            action,
                            shared_state,
                        );
                    }
                }
            }
            None
        }
        SettingsPage::EditRule(rule_id) => {
            if let Some(rp) = rules_page {
                if let Some(shared_state) = shared {
                    if let Some(org_service) = shared_state.services.organization_service.as_ref() {
                        let output = rp.render_edit_rule(ui, theme, *rule_id);
                        if let Some(data_action) = output.data_action {
                            let user_config = shared_state.signals().user_config.get();
                            let plugins = shared_state.plugin_ui_jobs.plugin_snapshot(&user_config);
                            crate::features::organization::presentation::views::rules_page::handle_rules_page_action(
                                rp,
                                data_action,
                                org_service,
                                plugins.as_deref().map(Vec::as_slice),
                            );
                        }
                        if let Some(editor_action) = output.editor_action {
                            use crate::features::organization::presentation::views::RuleEditorAction;
                            match editor_action {
                                RuleEditorAction::Saved | RuleEditorAction::Cancelled => {
                                    // Navigate back to organization rules list
                                    return Some(SettingsAction::NavigateTo(
                                        SettingsPage::OrganizationRules,
                                    ));
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
