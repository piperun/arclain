use crate::core::SettingsPage;
use crate::features::password_management::dialogs::PasswordRulesDialog;
use crate::features::settings::settings_content::{
    render_settings_content, ArchivesSettingsState, SecuritySettingsState, SettingsAction,
};
use crate::features::settings::settings_page::{
    render_breadcrumb, render_settings_header, render_settings_navigator, render_settings_overview,
};
use crate::shared::SharedState;
use eframe::egui;

pub struct SettingsFeature {
    pub security_state: SecuritySettingsState,
    pub archives_state: ArchivesSettingsState,
    pub password_rules_dialog: PasswordRulesDialog,
    pub plugins_state: crate::features::plugins::types::PluginsListState,
}

impl SettingsFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            security_state: SecuritySettingsState::default(),
            archives_state: ArchivesSettingsState::default(),
            password_rules_dialog: PasswordRulesDialog::default(),
            plugins_state: crate::features::plugins::types::PluginsListState::default(),
        }
    }

    pub fn render(
        &mut self,
        ctx: &egui::Context,
        shared: &SharedState,
        page: &mut SettingsPage,
        on_back: &mut bool,
        breadcrumb: Vec<(String, crate::core::AppPage)>,
        rules_page: Option<&mut crate::features::organization::rules_page::RulesPage>,
    ) {
        let mut action = None;

        egui::SidePanel::left("settings_nav")
            .resizable(false)
            .default_width(250.0)
            .show(ctx, |ui| {
                if let Some(new_page) = render_settings_navigator(ui, &shared.theme, page) {
                    *page = new_page;
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            // Render breadcrumb
            if let Some(target) = render_breadcrumb(ui, &shared.theme, &breadcrumb) {
                match target {
                    crate::core::AppPage::Settings(p) => *page = p,
                    crate::core::AppPage::Main => *on_back = true, // Navigate back/home
                    _ => {}
                }
            }
            ui.add_space(8.0);

            render_settings_header(ui, &shared.theme, page, on_back);
            ui.add_space(20.0);

            // If we are on a specific page, render content, otherwise render overview
            if *page == SettingsPage::Overview {
                if let Some(new_page) = render_settings_overview(ui, &shared.theme) {
                    *page = new_page;
                }
            } else {
                action = render_settings_content(
                    ui,
                    &shared.theme,
                    page,
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

        if let Some(action) = action {
            self.handle_action(action, shared);
        }
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
                state.cfg.cfg.temp_dir = temp_dir.map(std::path::PathBuf::from);
                if let Err(_e) = state.cfg.save() {
                    // self.archives_state.error = format!("Failed to save settings: {}", e);
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
        }
    }
}
