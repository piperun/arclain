//! Settings Page Header Rendering

use crate::core::SettingsPage;
use crate::features::settings::application::facade;
use crate::features::settings::domain::types::SettingsAction;
use crate::features::settings::presentation::pages::layout_editor;

use crate::features::settings::presentation::SettingsFeature;

use crate::shared::SharedState;
use eframe::egui;
use std::cell::Cell;

pub fn render_header(
    ui: &mut egui::Ui,
    feature: &mut SettingsFeature,
    hotkeys_feature: Option<&crate::features::hotkeys::HotkeysFeature>,
    password_management: Option<&crate::features::password_management::PasswordManagementFeature>,
    plugins_settings_state: Option<&mut crate::features::plugins::domain::types::PluginsListState>,
    shared: &SharedState,
    page: &SettingsPage,
) -> Option<SettingsAction> {
    let mut action = None;

    // Header Configuration Dispatch
    let install_clicked = Cell::new(false);
    let toolbar_save_clicked = Cell::new(false);
    let toolbar_reset_clicked = Cell::new(false);
    let info_panel_save_clicked = Cell::new(false);
    let info_panel_reset_clicked = Cell::new(false);
    let rule_cancel_clicked = Cell::new(false);
    let rule_save_clicked = Cell::new(false);

    // Fallback used when no PluginsFeature borrow is available — keeps
    // get_header_config callable without panicking. The fallback state
    // is throwaway: the Plugins page won't be reachable without
    // PluginsFeature wired in.
    let mut plugins_fallback = crate::features::plugins::domain::types::PluginsListState::default();
    let plugins_state_ref: &mut crate::features::plugins::domain::types::PluginsListState =
        plugins_settings_state.unwrap_or(&mut plugins_fallback);

    // Render the modal before the header borrows this state through its
    // retained closures. Its foreground layer still paints above the page.
    if let Some(pending) = plugins_state_ref.pending_install.as_mut() {
        match crate::features::plugins::presentation::views::render_plugin_install_dialog(
            ui.ctx(),
            &shared.theme,
            pending,
        ) {
            Some(
                crate::features::plugins::presentation::views::PluginInstallDialogResult::Cancel,
            ) => action = Some(SettingsAction::CancelPluginInstall),
            Some(
                crate::features::plugins::presentation::views::PluginInstallDialogResult::Install {
                    package_path,
                    expected_fingerprint,
                },
            ) => {
                action = Some(SettingsAction::ApprovePluginPackage {
                    package_path,
                    expected_fingerprint,
                })
            }
            None => {}
        }
    }

    let header_config =
        if *page == SettingsPage::Plugins {
            // Delegate to Plugins Page
            crate::features::plugins::presentation::pages::plugins_page::get_header_config(
                plugins_state_ref,
                page,
                &install_clicked,
                shared,
            )
        } else if *page == SettingsPage::ToolbarLayout {
            // Toolbar Layout Page header
            let has_changes = feature.toolbar_layout_state.dirty;
            crate::features::settings::presentation::views::header_config::
SettingsHeaderConfig::new("Toolbar Layout")
            .icon(egui_phosphor::regular::STACK.to_string())
            .description("Customize toolbar button layout")
            .has_changes(has_changes)
            .on_save(|| {
                toolbar_save_clicked.set(true);
            })
            .custom_actions(|ui| {
                if ui
                    .button(format!(
                        "{} Reset",
                        egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                    ))
                    .clicked()
                {
                    toolbar_reset_clicked.set(true);
                }
            })
        } else if *page == SettingsPage::InfoPanelLayout {
            // Info Panel Layout Page header
            let has_changes = feature.info_panel_layout_state.dirty;
            crate::features::settings::presentation::views::header_config::
SettingsHeaderConfig::new("Info Panel Layout")
            .icon(egui_phosphor::regular::SIDEBAR.to_string())
            .description("Customize info panel sections")
            .has_changes(has_changes)
            .on_save(|| {
                info_panel_save_clicked.set(true);
            })
            .custom_actions(|ui| {
                if ui
                    .button(format!(
                        "{} Reset",
                        egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                    ))
                    .clicked()
                {
                    info_panel_reset_clicked.set(true);
                }
            })
        } else if matches!(page, SettingsPage::EditRule(_)) {
            // Rule Editor - both Cancel and Save in header
            let title = if let SettingsPage::EditRule(id) = page {
                if *id == 0 {
                    "New Rule"
                } else {
                    "Edit Rule"
                }
            } else {
                "Edit Rule"
            };
            let is_dirty = feature.rule_editor_dirty;
            crate::features::settings::presentation::views::header_config::
SettingsHeaderConfig::new(title)
            .icon(page.icon())
            .description(page.description())
            .has_changes(is_dirty)
            .on_save(|| {
                rule_save_clicked.set(true);
            })
            .custom_actions(|ui| {
                if ui.add(
                    arclain_widgets::TextButton::new(
                        format!("{} Cancel", egui_phosphor::regular::X),
                        arclain_widgets::button::ButtonSize::Medium,
                    )
                    .variant(arclain_theme::ButtonVariant::Secondary)
                    .with_theme_colors(&shared.theme.colors)
                ).clicked() {
                    rule_cancel_clicked.set(true);
                }
            })
        } else {
            // Default Header for other pages
            crate::features::settings::presentation::views::header_config::
SettingsHeaderConfig::new(page.display_name())
            .icon(page.icon())
            .description(page.description())
            .has_changes(feature.check_changes(
                shared,
                page,
                crate::features::settings::SettingsFeatureRefs {
                    password_management,
                },
            ))
        };

    let crate::features::settings::presentation::views::header_config::SettingsHeaderConfig {
        title,
        icon,
        description,
        sub_description,
        has_changes,
        on_back,
        on_save,
        custom_actions,
        secondary_row,
        tertiary_row,
    } = header_config;
    let mut header = crate::shared::components::SettingsHeader::new(title).has_changes(has_changes);

    if let Some(icon) = icon {
        header = header.icon(icon);
    }
    if let Some(desc) = description {
        header = header.description(desc);
    }
    if let Some(sub_desc) = sub_description {
        header = header.sub_description(sub_desc);
    }
    if let Some(back) = on_back {
        header = header.on_back(back);
    }
    if let Some(row) = secondary_row {
        header = header.secondary_row(row);
    }
    if let Some(row) = tertiary_row {
        header = header.tertiary_row(row);
    }
    if let Some(actions) = custom_actions {
        header = header.custom_actions(actions);
    }

    // Save Action Logic
    if let Some(save_action) = on_save {
        header = header.on_save(save_action);
    } else if *page != SettingsPage::Plugins && !matches!(page, SettingsPage::EditRule(_)) {
        header = header.on_save(|| match page {
            SettingsPage::General => {
                action = Some(SettingsAction::SaveGeneral {
                    open_nested_in_new_tab: *feature.general_state.open_nested_in_new_tab.read(),
                    drop_behavior: *feature.general_state.drop_behavior.read(),
                    restore_tabs_on_launch: *feature.general_state.restore_tabs_on_launch.read(),
                });
            }
            SettingsPage::Archives => {
                let temp_dir_opt = if feature.archives_state.temp_dir.read().trim().is_empty() {
                    None
                } else {
                    Some(feature.archives_state.temp_dir.read().trim().to_string())
                };
                action = Some(SettingsAction::SaveArchives {
                    temp_dir: temp_dir_opt,
                });
            }
            SettingsPage::Security => {
                let key_opt = if feature
                    .security_state
                    .key_file_path
                    .read()
                    .trim()
                    .is_empty()
                {
                    None
                } else {
                    Some(
                        feature
                            .security_state
                            .key_file_path
                            .read()
                            .trim()
                            .to_string(),
                    )
                };
                let db_opt = if feature
                    .security_state
                    .secrets_db_path
                    .read()
                    .trim()
                    .is_empty()
                {
                    None
                } else {
                    Some(
                        feature
                            .security_state
                            .secrets_db_path
                            .read()
                            .trim()
                            .to_string(),
                    )
                };
                let policy_opt = Some(
                    feature
                        .security_state
                        .encrypted_crc_policy
                        .read()
                        .as_str()
                        .to_string(),
                );

                action = Some(SettingsAction::SaveSecurity {
                    key_file_path: key_opt,
                    secrets_db_path: db_opt,
                    encrypted_crc_policy: policy_opt,
                });
            }
            SettingsPage::PasswordRules => {
                if let Some(pm) = password_management {
                    action = Some(SettingsAction::SavePasswordRules {
                        rules: pm.password_rules_dialog.rules.clone(),
                    });
                }
            }
            SettingsPage::Network => {
                let address_opt = if feature
                    .network_state
                    .socks5_address
                    .read()
                    .trim()
                    .is_empty()
                {
                    None
                } else {
                    Some(
                        feature
                            .network_state
                            .socks5_address
                            .read()
                            .trim()
                            .to_string(),
                    )
                };
                let username_opt = if feature
                    .network_state
                    .socks5_username
                    .read()
                    .trim()
                    .is_empty()
                {
                    None
                } else {
                    Some(
                        feature
                            .network_state
                            .socks5_username
                            .read()
                            .trim()
                            .to_string(),
                    )
                };
                let password_opt = if feature.network_state.socks5_password.read().is_empty() {
                    None
                } else {
                    Some(feature.network_state.socks5_password.read().clone())
                };

                action = Some(SettingsAction::SaveNetwork {
                    socks5_enabled: *feature.network_state.socks5_enabled.read(),
                    socks5_address: address_opt,
                    socks5_username: username_opt,
                    socks5_password: password_opt,
                });
            }
            SettingsPage::KeyboardMouse => {
                if let Some(hf) = hotkeys_feature {
                    action = Some(SettingsAction::SaveKeyboardMouse {
                        bindings: hf.keyboard_mouse_state.to_config(),
                    });
                }
            }
            SettingsPage::Server => {
                let url_opt = if feature.server_state.url.read().trim().is_empty() {
                    None
                } else {
                    Some(feature.server_state.url.read().trim().to_string())
                };
                let api_key_opt = if feature.server_state.api_key.read().trim().is_empty() {
                    None
                } else {
                    Some(feature.server_state.api_key.read().clone())
                };
                action = Some(SettingsAction::SaveServer {
                    enabled: *feature.server_state.enabled.read(),
                    url: url_opt,
                    api_key: api_key_opt,
                });
            }
            _ => {}
        });
    }

    header.show(ui, &shared.theme);
    // Handle actions triggered by flags
    if install_clicked.get() {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter(
                crate::features::settings::domain::types::WIRT_PLUGIN_FILTER_NAME,
                crate::features::settings::domain::types::WIRT_PLUGIN_FILTER_SUFFIXES,
            )
            .set_title(crate::features::settings::domain::types::WIRT_PLUGIN_PICKER_TITLE)
            .pick_file()
        {
            action = Some(SettingsAction::InspectPluginPackage { package_path: file });
        }
    }

    if toolbar_save_clicked.get() {
        save_layout_editor(&mut feature.toolbar_layout_state, shared);
    }

    if toolbar_reset_clicked.get() {
        feature.toolbar_layout_state.loaded = false;
        feature.toolbar_layout_state.dirty = false;
    }

    if info_panel_save_clicked.get() {
        save_layout_editor(&mut feature.info_panel_layout_state, shared);
    }

    if info_panel_reset_clicked.get() {
        feature.info_panel_layout_state.loaded = false;
        feature.info_panel_layout_state.dirty = false;
    }

    if rule_cancel_clicked.get() {
        action = Some(SettingsAction::NavigateTo(SettingsPage::OrganizationRules));
    }

    if rule_save_clicked.get() {
        action = Some(SettingsAction::SaveEditedRule);
    }

    action
}

/// Saves one layout editor's arrangement and refreshes the canonical item
/// signals from the application, so the toolbar, info panel and context
/// menus around the app pick the change up on the next frame.
///
/// Both editors go through this rather than each refreshing only its own
/// region: they write into one store, the refresh is three cheap reads,
/// and the alternative -- the pre-facade split where the toolbar refreshed
/// one signal and the info panel refreshed all three -- was an asymmetry
/// with no reason behind it.
///
/// A failed save is reported rather than swallowed, and leaves the editor
/// dirty so the user can retry. The pre-facade version discarded the
/// result: a failed write cleared the dirty flag anyway and the user's
/// arrangement was silently lost.
fn save_layout_editor<R: layout_editor::Region>(
    state: &mut layout_editor::LayoutEditorState<R>,
    shared: &SharedState,
) {
    if let Err(error) = state.save(shared) {
        tracing::warn!("{error}");
        shared.toaster.lock().error(error);
        return;
    }
    let Some((app, runtime)) = facade::handles(shared) else {
        return;
    };
    shared.app_state.lock().reload_ui_config(app, runtime);
}
