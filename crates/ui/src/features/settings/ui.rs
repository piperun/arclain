use crate::core::SettingsPage;
use crate::features::password_management::dialogs::PasswordRulesDialog;
use crate::features::settings::settings_content::{
    render_settings_content, ArchivesSettingsState, GeneralSettingsState, SecuritySettingsState,
    SettingsAction,
};
use crate::features::settings::settings_page::{
    render_breadcrumb, render_settings_header, render_settings_navigator, render_settings_overview,
};
use crate::shared::SharedState;
use eframe::egui;

pub struct SettingsFeature {
    pub general_state: GeneralSettingsState,
    pub security_state: SecuritySettingsState,
    pub archives_state: ArchivesSettingsState,
    pub password_rules_dialog: PasswordRulesDialog,
    pub plugins_state: crate::features::plugins::types::PluginsListState,
}

impl SettingsFeature {
    pub fn new(shared: &SharedState) -> Self {
        // Load saved settings from config
        let open_nested_in_new_tab = {
            let state = shared.app_state.lock();
            state.user_config.open_nested_in_new_tab
        };

        Self {
            general_state: GeneralSettingsState {
                open_nested_in_new_tab,
            },
            security_state: SecuritySettingsState::default(),
            archives_state: ArchivesSettingsState::default(),
            password_rules_dialog: PasswordRulesDialog::default(),
            plugins_state: crate::features::plugins::types::PluginsListState::default(),
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
                            .fill(shared.theme.colors.bg_secondary)
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

                                        // Header
                                        let has_changes = self.check_changes(shared, page);
                                        if render_settings_header(ui, &shared.theme, page, has_changes) {
                                            // Handle global save
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
                                                action = render_settings_content(
                                                    ui,
                                                    &shared.theme,
                                                    page,
                                                    &mut self.general_state,
                                                    &mut self.security_state,
                                                    &mut self.archives_state,
                                                    &mut self.password_rules_dialog,
                                                    None, // Plugin manager not available in shared state yet?
                                                    &mut self.plugins_state,
                                                    rules_page,
                                                    &shared.app_state, // Need app state for DB check in rules page
                                                );
                                            }
                                        });
                                });
                            });
                    });
                });
            });

        if let Some(action) = action {
            self.handle_action(action, shared);
        }

        navigate_to
    }

    pub fn handle_action(&mut self, action: SettingsAction, shared: &SharedState) {
        match action {
            SettingsAction::SaveSecurity {
                key_file_path,
                secrets_db_path,
                encrypted_crc_policy,
            } => {
                let mut state = shared.app_state.lock();
                let key_file_str = key_file_path;
                let secrets_db_str = secrets_db_path;

                if let Err(e) =
                    state.apply_preferences(key_file_str, secrets_db_str, encrypted_crc_policy)
                {
                    self.security_state.error = format!("Failed to save settings: {}", e);
                } else {
                    self.security_state.info = "Settings saved successfully".to_string();
                }
            }
            SettingsAction::SaveArchives { temp_dir } => {
                let mut state = shared.app_state.lock();
                state.user_config.temp_dir = temp_dir;
                // Save via DB if available
                if let Some(ref dbs) = state.dbs {
                    let _ = dbs.config.with_connection(|conn| {
                        state.user_config.save(conn).ok();
                        Ok::<_, anyhow::Error>(())
                    });
                }
            }
            SettingsAction::MoveVault { dest_path } => {
                let mut state = shared.app_state.lock();
                if let Err(e) = state.move_vault(&dest_path) {
                    self.security_state.error = format!("Failed to move vault: {}", e);
                } else {
                    self.security_state.info = "Vault moved successfully".to_string();
                }
            }
            SettingsAction::RekeyVault { new_key_file_path } => {
                let mut state = shared.app_state.lock();
                if let Err(e) = state.rekey_vault(&new_key_file_path) {
                    self.security_state.error = format!("Failed to rekey vault: {}", e);
                } else {
                    self.security_state.info = "Vault rekeyed successfully".to_string();
                }
            }
            SettingsAction::SavePasswordRules { rules } => {
                let mut state = shared.app_state.lock();
                let core_rules = rules
                    .into_iter()
                    .map(|r| arclain_core::PassRule {
                        name: r.name,
                        pattern: r.pattern,
                        password: r.password,
                        priority: r.priority,
                        enabled: r.enabled,
                    })
                    .collect();
                if let Err(e) = state.save_password_rules(core_rules) {
                    // self.archives_state.error = format!("Failed to save rules: {}", e);
                    tracing::error!("Failed to save password rules: {}", e);
                }
            }
            SettingsAction::InstallPlugin { wasm_path } => {
                let state = shared.app_state.lock();
                if let Some(manager) = &state.plugin_manager {
                    let mut mgr = manager.lock();
                    match mgr.install_plugin(std::path::Path::new(&wasm_path)) {
                        Ok(id) => {
                            tracing::info!("Successfully installed plugin: {}", id);
                            // Refresh list
                            self.plugins_state.update_from_manager(&mgr);
                        }
                        Err(e) => {
                            tracing::error!("Failed to install plugin: {}", e);
                        }
                    }
                }
            }
            SettingsAction::ClearCacheIndex => {
                let mut state = shared.app_state.lock();
                if let Some(dbs) = &mut state.dbs {
                    // cache_index table is in metadata db (MetadataCacheDb)
                    if let Err(e) = dbs.metadata.clear_cache_index() {
                        self.archives_state.checksum_enabled = false; // Just to trigger a repaint/usage
                        tracing::error!("Failed to clear cache index: {}", e);
                    } else {
                        tracing::info!("Cache index cleared successfully");
                    }
                }
            }
            SettingsAction::ClearCacheContent => {
                let state = shared.app_state.lock();
                let cache_dir = if let Some(paths) = &state.db_paths {
                    paths
                        .cache_db
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join("content")
                } else {
                    // Fallback if DB not loaded (unlikely in settings)
                    std::path::PathBuf::from("data/content")
                };
                drop(state);

                // Perform in background
                std::thread::spawn(move || {
                    tracing::info!("Clearing cache content at {:?}", cache_dir);
                    if cache_dir.exists() {
                        if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
                            tracing::error!("Failed to remove cache dir: {}", e);
                        }
                        // Recreate
                        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                            tracing::error!("Failed to recreate cache dir: {}", e);
                        }
                    }
                });
            }
            SettingsAction::SaveGeneral {
                open_nested_in_new_tab,
            } => {
                let mut state = shared.app_state.lock();
                state.user_config.open_nested_in_new_tab = open_nested_in_new_tab;
                // Save via DB if available
                if let Some(ref dbs) = state.dbs {
                    if let Err(e) = dbs.config.with_connection(|conn| {
                        state.user_config.save(conn).ok();
                        Ok::<_, anyhow::Error>(())
                    }) {
                        tracing::error!("Failed to save general settings: {}", e);
                    } else {
                        tracing::info!("General settings saved");
                    }
                }
            }
        }
    }
}
