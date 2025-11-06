use super::dialogs::{
    zip_pass_rules::{PasswordRule, PasswordRulesDialog},
    EncryptedCrcPolicy,
};
use super::password_rules_page;
use super::theme::AppTheme;
use crate::app::navigation::SettingsPage;
use eframe::egui;

/// Actions that can be triggered from settings pages
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
pub fn render_archives_settings(ui: &mut egui::Ui, theme: &AppTheme) {
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
                ui.add_space(8.0);

                ui.label("Coming soon: Default extraction directory, overwrite policy");
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
        });
}

/// Render the Security settings page (migrated from preferences dialog)
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

            // Section: Encryption
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
    password_rules_dialog: &mut PasswordRulesDialog,
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
        SettingsPage::Archives => {
            render_archives_settings(ui, theme);
            None
        }
        SettingsPage::Security => render_security_settings(ui, theme, security_state),
        SettingsPage::PasswordRules => {
            render_password_rules_settings(ui, theme, password_rules_dialog)
        }
    }
}
