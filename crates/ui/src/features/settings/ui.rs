use crate::core::SettingsPage;
use crate::features::password_management::dialogs::PasswordRulesDialog;
use crate::features::settings::settings_content::{
    render_settings_content, ArchivesSettingsState, GeneralSettingsState, SecuritySettingsState,
    SettingsAction,
};
use crate::features::settings::pages::interface::InterfaceSettingsState;
use crate::features::settings::pages::{InfoPanelLayoutState, ToolbarLayoutState};
use crate::features::settings::settings_page::{
    render_breadcrumb, render_settings_navigator, render_settings_overview,
};
use crate::shared::SharedState;
use eframe::egui;

pub struct SettingsFeature {
    pub general_state: GeneralSettingsState,
    pub security_state: SecuritySettingsState,
    pub archives_state: ArchivesSettingsState,
    pub password_rules_dialog: PasswordRulesDialog,
    pub plugins_state: crate::features::plugins::types::PluginsListState,
    pub interface_state: InterfaceSettingsState,
    pub toolbar_layout_state: ToolbarLayoutState,
    pub info_panel_layout_state: InfoPanelLayoutState,
    pub last_visited_page: Option<SettingsPage>,
}

impl SettingsFeature {
    pub fn new(shared: &SharedState) -> Self {
        // Load saved settings from config
        let open_nested_in_new_tab = {
            let state = shared.app_state.lock();
            state.user_config.open_nested_in_new_tab
        };

        // Pre-load rules initially
        let rules = {
            let state = shared.app_state.lock();
            state.pass_rules.iter().map(|r| crate::features::password_management::dialogs::zip_pass_rules::PasswordRule {
                 name: r.name.clone(),
                 pattern: r.pattern.clone(),
                 password: r.password.clone(),
                 priority: r.priority,
                 enabled: r.enabled,
            }).collect()
        };

        Self {
            general_state: GeneralSettingsState {
                open_nested_in_new_tab,
            },
            security_state: SecuritySettingsState::default(),
            archives_state: ArchivesSettingsState::default(),
            password_rules_dialog: PasswordRulesDialog {
                rules,
                ..Default::default()
            },
            plugins_state: crate::features::plugins::types::PluginsListState::default(),
            interface_state: InterfaceSettingsState::default(),
            toolbar_layout_state: ToolbarLayoutState::default(),
            info_panel_layout_state: InfoPanelLayoutState::default(),
            last_visited_page: None,
        }
    }

    pub fn check_changes(&self, shared: &SharedState, page: &SettingsPage) -> bool {
        let state = shared.app_state.lock();

        match page {
            SettingsPage::General => {
                self.general_state.open_nested_in_new_tab
                    != state.user_config.open_nested_in_new_tab
            }
            SettingsPage::Archives => {
                // Compare temp_dir
                // Currently temp_dir depends on option. The UI state is a string. UserConfig is Option<String>.
                // Wait, UserConfig Definition of temp_dir? I suspect it is Option<PathBuf> or Option<String>.
                // Let's assume Option<String> based on SaveArchives handler: state.user_config.temp_dir = temp_dir;
                let current_val = if self.archives_state.temp_dir.trim().is_empty() {
                    None
                } else {
                    Some(self.archives_state.temp_dir.trim().to_string())
                };

                // We need to know what state.user_config.temp_dir is.
                // Based on `SettingsAction::SaveArchives { temp_dir }`: `state.user_config.temp_dir = temp_dir;`
                // And `temp_dir` in `SaveArchives` is `Option<String>`.
                // So state.user_config.temp_dir must be `Option<String>`.

                current_val != state.user_config.temp_dir
            }
            SettingsPage::Security => {
                // Check key file or secrets db
                !self.security_state.key_file_path.trim().is_empty() 
                || !self.security_state.secrets_db_path.trim().is_empty()
                // Policy - assume changed if not default
                || self.security_state.encrypted_crc_policy != crate::features::settings::types::EncryptedCrcPolicy::default()
            }
            SettingsPage::PasswordRules => {
                // Compare rules
                // self.password_rules_dialog.rules vs state.pass_rules
                // Need to convert state.pass_rules (PassRule) to PasswordRule (UiRule) for comparison?
                // Or just count/content.
                if self.password_rules_dialog.rules.len() != state.pass_rules.len() {
                    return true;
                }
                // Deep compare
                for (i, rule) in self.password_rules_dialog.rules.iter().enumerate() {
                    let other = &state.pass_rules[i];
                    if rule.name != other.name
                        || rule.pattern != other.pattern
                        || rule.password != other.password
                        || rule.priority != other.priority
                        || rule.enabled != other.enabled
                    {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        shared: &SharedState,
        page: &SettingsPage,
        breadcrumb: Vec<(String, crate::core::AppPage)>,
        rules_page: Option<&mut crate::features::settings::pages::RulesPage>,
        search_text: &str,
    ) -> Option<crate::core::AppPage> {
        // Sync rules if entering PasswordRules page
        if *page == SettingsPage::PasswordRules && self.last_visited_page.as_ref() != Some(page) {
             let state = shared.app_state.lock();
             self.password_rules_dialog.rules = state.pass_rules.iter().map(|r| crate::features::password_management::dialogs::zip_pass_rules::PasswordRule {
                 name: r.name.clone(),
                 pattern: r.pattern.clone(),
                 password: r.password.clone(),
                 priority: r.priority,
                 enabled: r.enabled,
            }).collect();
            tracing::debug!("Reloaded {} password rules from app state", self.password_rules_dialog.rules.len());
        }
        self.last_visited_page = Some(page.clone());

        let mut action = None;
        let mut navigate_to = None;

        use egui_extras::{Size, StripBuilder};

        // Main Horizontal Strip: Nav | Content
        StripBuilder::new(ui)
            .size(Size::exact(250.0)) // Navigation width
            .size(Size::remainder())  // Content width
            .horizontal(|mut strip| {
                // Strip 1: Navigation
                strip.cell(|ui| {
                    ui.push_id("settings_nav_strip", |ui| {
                        // Mimic SidePanel styling
                        egui::Frame::side_top_panel(ui.style())
                            .fill(shared.theme.colors.surface_variant)
                            .inner_margin(egui::Margin::symmetric(12, 8)) // Add some padding
                            .show(ui, |ui| {
                                ui.set_height(ui.available_height()); // Fill height
                                if let Some(new_page) = render_settings_navigator(ui, &shared.theme, page) {
                                    navigate_to = Some(crate::core::AppPage::Settings(new_page));
                                }
                            });
                    });
                });

                // Strip 2: Content (Header / Body)
                strip.cell(|ui| {
                    ui.push_id("settings_content_strip", |ui| {
                        StripBuilder::new(ui)
                            .size(Size::initial(80.0)) // Header height (approx)
                            .size(Size::remainder()) // Scrollable content
                            .vertical(|mut strip| {
                                // Sub-strip 1: Header
                                strip.cell(|ui| {
                                    ui.vertical(|ui| {
                                        // Render breadcrumb
                                        if let Some(target) = render_breadcrumb(ui, &shared.theme, &breadcrumb) {
                                            navigate_to = Some(target);
                                        }
                                        ui.add_space(8.0);

                                        // Header Configuration Dispatch
                                        let install_clicked = std::cell::Cell::new(false); // Captured by plugins header
                                        let toolbar_save_clicked = std::cell::Cell::new(false);
                                        let toolbar_reset_clicked = std::cell::Cell::new(false);
                                        let info_panel_save_clicked = std::cell::Cell::new(false);
                                        let info_panel_reset_clicked = std::cell::Cell::new(false);
                                        
                                        let header_config = if *page == SettingsPage::Plugins {
                                            // Delegate to Plugins Page
                                            crate::features::plugins::plugins_page::get_header_config(
                                                &mut self.plugins_state, 
                                                page, 
                                                &install_clicked
                                            )
                                        } else if *page == SettingsPage::ToolbarLayout {
                                            // Toolbar Layout Page header
                                            let has_changes = self.toolbar_layout_state.dirty;
                                            crate::features::settings::header_config::SettingsHeaderConfig::new("Toolbar Layout")
                                                .icon(egui_phosphor::regular::STACK.to_string())
                                                .description("Customize toolbar button layout")
                                                .has_changes(has_changes)
                                                .on_save(|| {
                                                    toolbar_save_clicked.set(true);
                                                })
                                                .custom_actions(|ui| {
                                                    if ui.button(format!("{} Reset", egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE)).clicked() {
                                                        toolbar_reset_clicked.set(true);
                                                    }
                                                })
                                        } else if *page == SettingsPage::InfoPanelLayout {
                                            // Info Panel Layout Page header
                                            let has_changes = self.info_panel_layout_state.dirty;
                                            crate::features::settings::header_config::SettingsHeaderConfig::new("Info Panel Layout")
                                                .icon(egui_phosphor::regular::SIDEBAR.to_string())
                                                .description("Customize info panel sections")
                                                .has_changes(has_changes)
                                                .on_save(|| {
                                                    info_panel_save_clicked.set(true);
                                                })
                                                .custom_actions(|ui| {
                                                    if ui.button(format!("{} Reset", egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE)).clicked() {
                                                        info_panel_reset_clicked.set(true);
                                                    }
                                                })
                                        } else {
                                            // Default Header for other pages
                                            crate::features::settings::header_config::SettingsHeaderConfig::new(page.display_name())
                                                .icon(page.icon())
                                                .description(page.description())
                                                .has_changes(self.check_changes(shared, page))
                                        };

                                        // Render Header
                                        // We handle show_header logic: plugins list hides it if detail is shown...
                                        // Wait, my get_header_config for plugins HANDLES the detail view header!
                                        // So we ALWAYS show header if config is returned.
                                        // But wait, the previous logic HID the header if selected_plugin was some, 
                                        // then showed a DIFFERENT header.
                                        // The new get_header_config handles BOTH cases (List vs Detail).
                                        // So we just render what it returns.

                                        // Capture signals from config before consuming it?
                                        // Closures in config are `FnOnce`. We execute `header.show()`.
                                        
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
                                        // If config provides on_save, use that directly
                                        // Otherwise, for standard pages (non-Plugins), use built-in save logic
                                        if let Some(save_action) = header_config.on_save {
                                            header = header.on_save(save_action);
                                        } else if *page != SettingsPage::Plugins { 
                                             header = header.on_save(|| {
                                                match page {
                                                    SettingsPage::General => {
                                                        action = Some(SettingsAction::SaveGeneral {
                                                            open_nested_in_new_tab: self.general_state.open_nested_in_new_tab,
                                                        });
                                                    }
                                                    SettingsPage::Archives => {
                                                        let temp_dir_opt = if self.archives_state.temp_dir.trim().is_empty() {
                                                            None
                                                        } else {
                                                            Some(self.archives_state.temp_dir.trim().to_string())
                                                        };
                                                        action = Some(SettingsAction::SaveArchives {
                                                            temp_dir: temp_dir_opt,
                                                        });
                                                    }
                                                    SettingsPage::Security => {
                                                        let key_opt = if self.security_state.key_file_path.trim().is_empty() {
                                                            None
                                                        } else {
                                                            Some(self.security_state.key_file_path.trim().to_string())
                                                        };
                                                        let db_opt = if self.security_state.secrets_db_path.trim().is_empty() {
                                                            None
                                                        } else {
                                                            Some(self.security_state.secrets_db_path.trim().to_string())
                                                        };
                                                        let policy_opt = Some(self.security_state.encrypted_crc_policy.as_str().to_string());

                                                        action = Some(SettingsAction::SaveSecurity {
                                                            key_file_path: key_opt,
                                                            secrets_db_path: db_opt,
                                                            encrypted_crc_policy: policy_opt,
                                                        });
                                                    }
                                                    SettingsPage::PasswordRules => {
                                                         action = Some(SettingsAction::SavePasswordRules {
                                                            rules: self.password_rules_dialog.rules.clone(),
                                                        });
                                                    }
                                                    _ => {}
                                                }
                                             });
                                        }

                                        header.show(ui, &shared.theme);
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
                                        
                                        // Handle toolbar layout save
                                        if toolbar_save_clicked.get() {
                                            let state_guard = shared.app_state.lock();
                                            if let Some(dbs) = &state_guard.dbs {
                                                let _ = dbs.config.with_connection(|conn| {
                                                    self.toolbar_layout_state.save_to_db(conn);
                                                    Ok::<_, anyhow::Error>(())
                                                });
                                            }
                                            drop(state_guard);
                                            // Reload main toolbar
                                            let state_guard = shared.app_state.lock();
                                            if let Some(dbs) = &state_guard.dbs {
                                                if let Ok(items) = dbs.config.with_connection(|conn| {
                                                    arclain_db::list_items_by_region(conn, arclain_db::UiRegion::Toolbar)
                                                }) {
                                                    drop(state_guard);
                                                    shared.app_state.lock().toolbar_items = items;
                                                }
                                            }
                                        }
                                        
                                        // Handle toolbar layout reset
                                        if toolbar_reset_clicked.get() {
                                            self.toolbar_layout_state.loaded = false;
                                            self.toolbar_layout_state.dirty = false;
                                        }
                                        
                                        // Handle info panel layout save
                                        if info_panel_save_clicked.get() {
                                            let state_guard = shared.app_state.lock();
                                            if let Some(dbs) = &state_guard.dbs {
                                                let _ = dbs.config.with_connection(|conn| {
                                                    self.info_panel_layout_state.save_to_db(conn);
                                                    Ok::<_, anyhow::Error>(())
                                                });
                                            }
                                            // Reload UI config to update main window
                                            drop(state_guard);
                                            shared.app_state.lock().reload_ui_config();
                                        }
                                        
                                        // Handle info panel layout reset
                                        if info_panel_reset_clicked.get() {
                                            self.info_panel_layout_state.loaded = false;
                                            self.info_panel_layout_state.dirty = false;
                                        }
                                        
                                        ui.add_space(20.0);
                                    });
                                });

                                // Sub-strip 2: Scrollable Content
                                strip.cell(|ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt("settings_content_scroll")
                                        .show(ui, |ui| {
                                            // Force width to allow wrapping - explicit width from available space
                                            ui.set_width(ui.available_width());

                                            // If search is active, show results
                                            if !search_text.trim().is_empty() {
                                                if let Some(target) =
                                                    crate::features::settings::settings_page::render_settings_search_results(
                                                        ui,
                                                        &shared.theme,
                                                        search_text,
                                                    )
                                                {
                                                    navigate_to = Some(crate::core::AppPage::Settings(target));
                                                }
                                            } else if *page == SettingsPage::Overview {
                                                if let Some(new_page) = render_settings_overview(ui, &shared.theme) {
                                                    navigate_to = Some(crate::core::AppPage::Settings(new_page));
                                                }
                                            } else {
                                                // Access plugin manager safely
                                                let pm_arc_opt = shared.app_state.lock().plugin_manager.clone();
                                                let pm_guard = pm_arc_opt.as_ref().map(|m| m.lock());

                                                action = render_settings_content(
                                                    ui,
                                                    &shared.theme,
                                                    page,
                                                    &mut self.general_state,
                                                    &mut self.security_state,
                                                    &mut self.archives_state,
                                                    &mut self.password_rules_dialog,
                                                    pm_guard.as_deref(),
                                                    &mut self.plugins_state,
                                                    rules_page,
                                                    &mut self.interface_state,
                                                    &mut self.toolbar_layout_state,
                                                    &mut self.info_panel_layout_state,
                                                    &shared.app_state,
                                                    Some(shared),
                                                );
                                            }
                                        });
                                });
                            });
                    });
                });
            });

        if let Some(action) = action {
            // Check if this is a navigation action
            if let Some(target_page) = crate::features::settings::action_handlers::extract_navigation(&action) {
                navigate_to = Some(crate::core::AppPage::Settings(target_page));
            } else {
                // Handle non-navigation actions
                self.handle_action(action, shared);
            }
        }

        navigate_to
    }

    pub fn handle_action(&mut self, action: SettingsAction, shared: &SharedState) {
        crate::features::settings::action_handlers::handle_action(
            action,
            &mut self.security_state,
            &mut self.archives_state,
            &mut self.plugins_state,
            shared,
        );
    }
}
