//! Settings Page Header Rendering

use crate::core::SettingsPage;
use crate::features::settings::types::SettingsAction;
use crate::features::settings::views::SettingsFeature;
use crate::shared::SharedState;
use eframe::egui;
use std::cell::Cell;

pub fn render_header(
    ui: &mut egui::Ui,
    feature: &mut SettingsFeature,
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

    let header_config = if *page == SettingsPage::Plugins {
        // Delegate to Plugins Page
        crate::features::plugins::plugins_page::get_header_config(
            &mut feature.plugins_state,
            page,
            &install_clicked,
        )
    } else if *page == SettingsPage::ToolbarLayout {
        // Toolbar Layout Page header
        let has_changes = feature.toolbar_layout_state.dirty;
        crate::features::settings::header_config::SettingsHeaderConfig::new("Toolbar Layout")
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
        crate::features::settings::header_config::SettingsHeaderConfig::new("Info Panel Layout")
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
    } else {
        // Default Header for other pages
        crate::features::settings::header_config::SettingsHeaderConfig::new(page.display_name())
            .icon(page.icon())
            .description(page.description())
            .has_changes(feature.check_changes(shared, page))
    };

    let mut header = crate::shared::components::SettingsHeader::new(header_config.title)
        .has_changes(header_config.has_changes);

    if let Some(icon) = header_config.icon {
        header = header.icon(icon);
    }
    if let Some(desc) = header_config.description {
        header = header.description(desc);
    }
    if let Some(sub_desc) = header_config.sub_description {
        header = header.sub_description(sub_desc);
    }
    if let Some(back) = header_config.on_back {
        header = header.on_back(back);
    }
    if let Some(row) = header_config.secondary_row {
        header = header.secondary_row(row);
    }
    if let Some(row) = header_config.tertiary_row {
        header = header.tertiary_row(row);
    }
    if let Some(actions) = header_config.custom_actions {
        header = header.custom_actions(actions);
    }

    // Save Action Logic
    if let Some(save_action) = header_config.on_save {
        header = header.on_save(save_action);
    } else if *page != SettingsPage::Plugins {
        header = header.on_save(|| match page {
            SettingsPage::General => {
                action = Some(SettingsAction::SaveGeneral {
                    open_nested_in_new_tab: *feature.general_state.open_nested_in_new_tab.read(),
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
                action = Some(SettingsAction::SavePasswordRules {
                    rules: feature.password_rules_dialog.rules.clone(),
                });
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
            _ => {}
        });
    }

    header.show(ui, &shared.theme);

    // Handle actions triggered by flags
    if install_clicked.get() {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("WASM Plugin", &["wasm"])
            .set_title("Select Plugin to Install")
            .pick_file()
        {
            action = Some(SettingsAction::InstallPlugin {
                wasm_path: file.to_string_lossy().to_string(),
            });
        }
    }

    if toolbar_save_clicked.get() {
        if let Some(ui_service) = shared.services.ui_service.as_ref() {
            feature.toolbar_layout_state.save_to_service(ui_service);
            if let Ok(items) = ui_service.list_toolbar_items() {
                let state_guard = shared.app_state.lock();
                state_guard.signals.toolbar_items.set(items);
            }
        }
    }

    if toolbar_reset_clicked.get() {
        feature.toolbar_layout_state.loaded = false;
        feature.toolbar_layout_state.dirty = false;
    }

    if info_panel_save_clicked.get() {
        if let Some(ui_service) = shared.services.ui_service.as_ref() {
            feature.info_panel_layout_state.save_to_service(ui_service);
            shared.app_state.lock().reload_ui_config(ui_service);
        }
    }

    if info_panel_reset_clicked.get() {
        feature.info_panel_layout_state.loaded = false;
        feature.info_panel_layout_state.dirty = false;
    }

    action
}
