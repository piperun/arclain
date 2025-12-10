use crate::core::SettingsPage;
use crate::features::password_management::dialogs::zip_pass_rules::{
    PasswordRule, PasswordRulesDialog,
};
use crate::features::password_management::rules_page as password_rules_page;
use crate::features::plugins::plugin_list;
use crate::features::plugins::types::PluginsListState;
use crate::shared::theme::AppTheme;
use arclain_plugins::PluginManager;
use eframe::egui;

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum EncryptedCrcPolicy {
    OnOpen,
    PromptOnOpen,
    OnAccess,
}

impl Default for EncryptedCrcPolicy {
    fn default() -> Self {
        Self::OnOpen
    }
}

impl EncryptedCrcPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            EncryptedCrcPolicy::OnOpen => "on_open",
            EncryptedCrcPolicy::PromptOnOpen => "prompt_on_open",
            EncryptedCrcPolicy::OnAccess => "on_access",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            EncryptedCrcPolicy::OnOpen => "When opening archive",
            EncryptedCrcPolicy::PromptOnOpen => "Prompt when opening archive",
            EncryptedCrcPolicy::OnAccess => "When opening/editing file",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SettingsAction {
    /// Save security settings
    SaveSecurity {
        key_file_path: Option<String>,
        secrets_db_path: Option<String>,
        encrypted_crc_policy: Option<String>,
    },
    /// Move vault to new location
    MoveVault { dest_path: String },
    /// Rekey vault with new key
    RekeyVault { new_key_file_path: String },
    /// Save password rules
    SavePasswordRules { rules: Vec<PasswordRule> },
    /// Save archives settings
    SaveArchives { temp_dir: Option<String> },
    /// Install a plugin from a .wasm file
    InstallPlugin { wasm_path: String },
    /// Clear the cache index (database entries)
    ClearCacheIndex,
    /// Clear the cache content (files on disk)
    ClearCacheContent,
}

/// State for the security settings page
#[derive(Default)]
pub struct SecuritySettingsState {
    pub key_file_path: String,
    pub secrets_db_path: String,
    pub encrypted_crc_policy: EncryptedCrcPolicy,
    pub info: String,
    pub error: String,
}

/// State for the archives settings page
#[derive(Default)]
pub struct ArchivesSettingsState {
    pub temp_dir: String,
    // Checksum settings
    pub checksum_enabled: bool,
    pub checksum_mode: ChecksumMode,
    pub checksum_algorithm: ChecksumAlgorithm,
    pub verify_after_extract: bool,
    pub verify_after_organize: bool,
}

/// Checksum verification mode (mirrors VerifyMode)
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ChecksumMode {
    #[default]
    Simple,
    Full,
}

impl ChecksumMode {
    pub fn display_name(&self) -> &'static str {
        match self {
            ChecksumMode::Simple => "Simple (root hash only)",
            ChecksumMode::Full => "Full (all file hashes)",
        }
    }
}

/// Checksum algorithm selection
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ChecksumAlgorithm {
    #[default]
    Crc32,
    XxHash,
    Sha256,
}

impl ChecksumAlgorithm {
    pub fn display_name(&self) -> &'static str {
        match self {
            ChecksumAlgorithm::Crc32 => "CRC32 (fastest)",
            ChecksumAlgorithm::XxHash => "XXHash (fast, modern)",
            ChecksumAlgorithm::Sha256 => "SHA-256 (secure, slower)",
        }
    }
}

/// Render the General settings page
pub fn render_general_settings(ui: &mut egui::Ui, theme: &AppTheme) {
    egui::ScrollArea::vertical()
        .id_salt("general_settings_scroll")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

            // Section: Appearance
            render_settings_section(ui, theme, "Appearance", |ui| {
                ui.label(
                    egui::RichText::new("Theme settings and visual preferences")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.add_space(8.0);

                ui.label("Coming soon: Theme customization options");
            });

            ui.add_space(8.0);

            // Section: Behavior
            render_settings_section(ui, theme, "Behavior", |ui| {
                ui.label(
                    egui::RichText::new("Application behavior and default actions")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.add_space(8.0);

                ui.label("Coming soon: Default extraction location, file associations");
            });
        });
}

/// Render the Archives settings page
pub fn render_archives_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ArchivesSettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    egui::ScrollArea::vertical()
        .id_salt("archives_settings_scroll")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

            // Section: Extraction
            render_settings_section(ui, theme, "Extraction", |ui| {
                ui.label(
                    egui::RichText::new("Configure how files are extracted from archives")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.label(
                    egui::RichText::new(
                        "Directory used for intermediate operations (like conversion)",
                    )
                    .size(12.0)
                    .color(theme.colors.text_secondary),
                );
                ui.add_space(8.0);

                ui.label(
                    egui::RichText::new("Temporary Directory")
                        .size(12.0)
                        .strong()
                        .color(theme.colors.text_primary),
                );
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let te =
                        egui::TextEdit::singleline(&mut state.temp_dir).hint_text("System Default");
                    ui.add_sized([ui.available_width() - 110.0, 28.0], te);
                    if ui
                        .add(egui::Button::new("Browse…").min_size(egui::vec2(100.0, 28.0)))
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            state.temp_dir = path.to_string_lossy().to_string();
                        }
                    }
                });
            });

            ui.add_space(8.0);

            // Section: Compression
            render_settings_section(ui, theme, "Compression", |ui| {
                ui.label(
                    egui::RichText::new("Settings for creating and modifying archives")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.add_space(8.0);

                ui.label("Coming soon: Compression level, format preferences");
            });

            ui.add_space(8.0);

            // Section: Integrity Verification
            render_settings_section(ui, theme, "Integrity Verification", |ui| {
                ui.label(
                    egui::RichText::new(
                        "Verify file integrity after extraction and organization operations",
                    )
                    .size(12.0)
                    .color(theme.colors.text_secondary),
                );
                ui.add_space(12.0);

                // Enable checkbox
                ui.checkbox(&mut state.checksum_enabled, "Enable integrity verification");
                ui.add_space(8.0);

                // Only show options if enabled
                if state.checksum_enabled {
                    // Mode selector
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Verification Mode:")
                                .size(12.0)
                                .color(theme.colors.text_primary),
                        );
                        egui::ComboBox::new("checksum_mode", "")
                            .selected_text(state.checksum_mode.display_name())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut state.checksum_mode,
                                    ChecksumMode::Simple,
                                    ChecksumMode::Simple.display_name(),
                                );
                                ui.selectable_value(
                                    &mut state.checksum_mode,
                                    ChecksumMode::Full,
                                    ChecksumMode::Full.display_name(),
                                );
                            });
                    });
                    ui.add_space(4.0);

                    // Algorithm selector
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Algorithm:")
                                .size(12.0)
                                .color(theme.colors.text_primary),
                        );
                        egui::ComboBox::new("checksum_algorithm", "")
                            .selected_text(state.checksum_algorithm.display_name())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut state.checksum_algorithm,
                                    ChecksumAlgorithm::Crc32,
                                    ChecksumAlgorithm::Crc32.display_name(),
                                );
                                ui.selectable_value(
                                    &mut state.checksum_algorithm,
                                    ChecksumAlgorithm::XxHash,
                                    ChecksumAlgorithm::XxHash.display_name(),
                                );
                                ui.selectable_value(
                                    &mut state.checksum_algorithm,
                                    ChecksumAlgorithm::Sha256,
                                    ChecksumAlgorithm::Sha256.display_name(),
                                );
                            });
                    });
                    ui.add_space(8.0);

                    // Verification triggers
                    ui.checkbox(&mut state.verify_after_extract, "Verify after extraction");
                    ui.checkbox(&mut state.verify_after_organize, "Verify after organize");
                }
            });

            ui.add_space(16.0);

            // Save button
            ui.horizontal(|ui| {
                let save_btn = egui::Button::new(egui::RichText::new("💾 Save Changes").strong())
                    .min_size(egui::vec2(140.0, 36.0));

                if ui.add(save_btn).clicked() {
                    let temp_dir_opt = if state.temp_dir.trim().is_empty() {
                        None
                    } else {
                        Some(state.temp_dir.trim().to_string())
                    };

                    action = Some(SettingsAction::SaveArchives {
                        temp_dir: temp_dir_opt,
                    });
                }
            });

            ui.add_space(8.0);
            
            // Section: Cache Management
            render_settings_section(ui, theme, "Cache Management", |ui| {
                ui.label(
                    egui::RichText::new("Manage the application cache (thumbnails, metadata, etc.)")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.add_space(8.0);
                
                ui.horizontal(|ui| {
                     if ui.button("Clear Cache Index").clicked() {
                         action = Some(SettingsAction::ClearCacheIndex);
                     }
                     
                     if ui.button("Clear Cache Content").clicked() {
                         action = Some(SettingsAction::ClearCacheContent);
                     }
                });
                ui.label(
                    egui::RichText::new("Clearing index removes database entries. Clearing content removes files from disk.")
                        .size(10.0)
                        .italics()
                        .color(theme.colors.text_secondary),
                );
            });
        });

    action
}

/// Render the Security settings page
pub fn render_security_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut SecuritySettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    egui::ScrollArea::vertical()
        .id_salt("security_settings_scroll")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

            render_settings_section(ui, theme, "Encryption", |ui| {
                ui.label(
                    egui::RichText::new("Master key and secrets database configuration")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.add_space(12.0);

                // Key file picker
                ui.label(
                    egui::RichText::new("Master key file (32-byte raw / hex / base64)")
                        .size(12.0)
                        .strong()
                        .color(theme.colors.text_primary),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let te = egui::TextEdit::singleline(&mut state.key_file_path)
                        .hint_text("Select a key file...");
                    ui.add_sized([ui.available_width() - 110.0, 28.0], te);
                    if ui
                        .add(egui::Button::new("Browse…").min_size(egui::vec2(100.0, 28.0)))
                        .clicked()
                    {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            state.key_file_path = file.to_string_lossy().to_string();
                        }
                    }
                });

                ui.add_space(12.0);

                // Secrets DB picker
                ui.label(
                    egui::RichText::new("Secrets database (redb with AES-256-GCM)")
                        .size(12.0)
                        .strong()
                        .color(theme.colors.text_primary),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let te = egui::TextEdit::singleline(&mut state.secrets_db_path)
                        .hint_text("Path to pass.redb...");
                    ui.add_sized([ui.available_width() - 110.0, 28.0], te);
                    if ui
                        .add(egui::Button::new("Browse…").min_size(egui::vec2(100.0, 28.0)))
                        .clicked()
                    {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            state.secrets_db_path = file.to_string_lossy().to_string();
                        }
                    }
                });
            });

            ui.add_space(8.0);

            // Section: CRC Policy
            render_settings_section(ui, theme, "CRC Computation", |ui| {
                ui.label(
                    egui::RichText::new("When to compute CRC checksums for encrypted files")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.add_space(8.0);

                egui::ComboBox::new("crc_policy", "Encrypted CRC computation")
                    .selected_text(state.encrypted_crc_policy.display_name())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut state.encrypted_crc_policy,
                            EncryptedCrcPolicy::OnOpen,
                            EncryptedCrcPolicy::OnOpen.display_name(),
                        );
                        ui.selectable_value(
                            &mut state.encrypted_crc_policy,
                            EncryptedCrcPolicy::PromptOnOpen,
                            EncryptedCrcPolicy::PromptOnOpen.display_name(),
                        );
                        ui.selectable_value(
                            &mut state.encrypted_crc_policy,
                            EncryptedCrcPolicy::OnAccess,
                            EncryptedCrcPolicy::OnAccess.display_name(),
                        );
                    });
            });

            ui.add_space(8.0);

            // Section: Vault Management
            render_settings_section(ui, theme, "Vault Management", |ui| {
                ui.label(
                    egui::RichText::new("Move or rekey your encrypted secrets vault")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    let move_btn =
                        egui::Button::new("Move vault…").min_size(egui::vec2(120.0, 32.0));
                    if ui.add(move_btn).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("pass.redb")
                            .save_file()
                        {
                            action = Some(SettingsAction::MoveVault {
                                dest_path: path.to_string_lossy().to_string(),
                            });
                        }
                    }

                    ui.add_space(8.0);

                    let rekey_btn =
                        egui::Button::new("Rekey vault…").min_size(egui::vec2(120.0, 32.0));
                    if ui.add(rekey_btn).clicked() {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            action = Some(SettingsAction::RekeyVault {
                                new_key_file_path: file.to_string_lossy().to_string(),
                            });
                        }
                    }
                });
            });

            ui.add_space(16.0);

            // Info / Error messages
            if !state.info.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(56, 142, 60), &state.info);
            }
            if !state.error.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &state.error);
            }

            ui.add_space(16.0);

            // Save button
            ui.horizontal(|ui| {
                let save_btn = egui::Button::new(egui::RichText::new("💾 Save Changes").strong())
                    .min_size(egui::vec2(140.0, 36.0));

                if ui.add(save_btn).clicked() {
                    let key_opt = if state.key_file_path.trim().is_empty() {
                        None
                    } else {
                        Some(state.key_file_path.trim().to_string())
                    };
                    let db_opt = if state.secrets_db_path.trim().is_empty() {
                        None
                    } else {
                        Some(state.secrets_db_path.trim().to_string())
                    };
                    let policy_opt = Some(state.encrypted_crc_policy.as_str().to_string());

                    action = Some(SettingsAction::SaveSecurity {
                        key_file_path: key_opt,
                        secrets_db_path: db_opt,
                        encrypted_crc_policy: policy_opt,
                    });
                }
            });
        });

    action
}

/// Render the Password Rules settings page (now renders the full page view)
/// Returns SavePasswordRules action if save button was clicked
pub fn render_password_rules_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    password_rules_dialog: &mut PasswordRulesDialog,
) -> Option<SettingsAction> {
    // Render the full password rules management page directly
    if let Some(password_rules_page::PasswordRulesPageResult::Save) =
        password_rules_page::render_password_rules_page(ui, theme, password_rules_dialog)
    {
        return Some(SettingsAction::SavePasswordRules {
            rules: password_rules_dialog.rules.clone(),
        });
    }
    None
}

/// Helper function to render a settings section with consistent styling
fn render_settings_section<R>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(theme.colors.bg_secondary)
        .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
        .corner_radius(8.0)
        .inner_margin(20.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(15.0)
                        .strong()
                        .color(theme.colors.text_primary),
                );
                ui.add_space(8.0);
                content(ui)
            })
            .inner
        })
        .inner
}

/// Render the appropriate settings content based on the current page
pub fn render_settings_content(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    page: &SettingsPage,
    security_state: &mut SecuritySettingsState,
    archives_state: &mut ArchivesSettingsState,
    password_rules_dialog: &mut PasswordRulesDialog,
    plugin_manager: Option<&PluginManager>,
    plugins_state: &mut PluginsListState,
    rules_page: Option<&mut crate::features::settings::pages::RulesPage>,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
) -> Option<SettingsAction> {
    match page {
        SettingsPage::Overview => {
            // This shouldn't be called as overview has its own rendering
            None
        }
        SettingsPage::General => {
            render_general_settings(ui, theme);
            None
        }
        SettingsPage::Interface => {
            crate::features::settings::pages::render_interface_settings(ui, theme, app_state);
            None
        }
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
                    // Generic way: db is ConfigDb which (since it wraps SqliteDb or is SqliteDb?)
                    // Wait, db.config IS SqliteDb according to previous error.
                    // But here I cloned it.
                    // So db is SqliteDb.
                    rp.render(ui, &db);
                } else {
                    ui.label("Database not available (encrypted?)");
                }
            } else {
                ui.label("Rules page not available.");
            }
            None
        }
        SettingsPage::Plugins => render_plugins_settings(ui, theme, plugin_manager, plugins_state),
    }
}

/// Render the Plugins settings page
pub fn render_plugins_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    plugin_manager: Option<&PluginManager>,
    plugins_state: &mut PluginsListState,
) -> Option<SettingsAction> {
    // Update plugin list from manager if available
    if let Some(manager) = plugin_manager {
        plugins_state.update_from_manager(manager);
    }

    // Render the plugin list
    if let Some(action) = plugin_list::render(ui, theme, plugins_state) {
        // Handle plugin actions
        return match action {
            plugin_list::PluginAction::SelectPlugin(id) => {
                plugins_state.selected_plugin = Some(id);
                None
            }
            plugin_list::PluginAction::EnablePlugin(id) => {
                if let Some(manager) = plugin_manager {
                    match manager.enable_plugin(&id) {
                        Ok(()) => {
                            tracing::info!("Plugin enabled: {}", id);
                            // Update the state immediately
                            plugins_state.update_from_manager(manager);
                        }
                        Err(e) => {
                            tracing::error!("Failed to enable plugin {}: {}", id, e);
                        }
                    }
                }
                None
            }
            plugin_list::PluginAction::DisablePlugin(id) => {
                if let Some(manager) = plugin_manager {
                    match manager.disable_plugin(&id) {
                        Ok(()) => {
                            tracing::info!("Plugin disabled: {}", id);
                            // Update the state immediately
                            plugins_state.update_from_manager(manager);
                        }
                        Err(e) => {
                            tracing::error!("Failed to disable plugin {}: {}", id, e);
                        }
                    }
                }
                None
            }
            plugin_list::PluginAction::ShowPluginSettings(id) => {
                // TODO: Implement settings dialog
                tracing::info!("Show settings for plugin: {}", id);
                None
            }
            plugin_list::PluginAction::InstallPlugin => {
                // Show file picker for .wasm files
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("WASM Plugin", &["wasm"])
                    .set_title("Select Plugin to Install")
                    .pick_file()
                {
                    tracing::info!("Selected plugin file: {}", file.display());
                    // Return action to be handled at app level where we have mutable access
                    Some(SettingsAction::InstallPlugin {
                        wasm_path: file.to_string_lossy().to_string(),
                    })
                } else {
                    None
                }
            }
        };
    }

    None
}
