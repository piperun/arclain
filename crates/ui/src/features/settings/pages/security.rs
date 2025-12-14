//! Security Settings Page
//!
//! Contains settings for encryption, CRC policy, and vault management.

use crate::features::settings::types::{EncryptedCrcPolicy, SecuritySettingsState, SettingsAction};
use crate::shared::theme::AppTheme;
use eframe::egui;

use super::render_settings_section;

/// Render the Security settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut SecuritySettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        render_settings_section(ui, theme, "Encryption", |ui| {
            ui.label(
                egui::RichText::new("Master key and secrets database configuration")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(12.0);

            // Calculate default paths for hints
            let defaults = arclain_core::config::database::DbPaths::defaults("arclain").ok();
            let default_key = defaults
                .as_ref()
                .and_then(|d| d.key_file.as_ref())
                .map(|p| p.to_string_lossy());
            let default_db = defaults.as_ref().map(|d| d.secrets_db.to_string_lossy());

            // Key file picker
            ui.label(
                egui::RichText::new("Master key file (32-byte raw / hex / base64)")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.on_surface),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let hint = default_key.as_deref().unwrap_or("Select a key file...");
                let te = egui::TextEdit::singleline(&mut state.key_file_path).hint_text(hint);
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
            if state.key_file_path.trim().is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "Default: System AppData location ({})",
                        default_key.as_deref().unwrap_or("Unknown")
                    ))
                    .size(11.0)
                    .color(theme.colors.on_surface_variant)
                    .italics(),
                );
            }

            ui.add_space(12.0);

            // Secrets DB picker
            ui.label(
                egui::RichText::new("Secrets database (redb with AES-256-GCM)")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.on_surface),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let hint = default_db.as_deref().unwrap_or("Path to pass.redb...");
                let te = egui::TextEdit::singleline(&mut state.secrets_db_path).hint_text(hint);
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
            if state.secrets_db_path.trim().is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "Default: System AppData location ({})",
                        default_db.as_deref().unwrap_or("Unknown")
                    ))
                    .size(11.0)
                    .color(theme.colors.on_surface_variant)
                    .italics(),
                );
            }
        });

        ui.add_space(8.0);

        // Section: CRC Policy
        render_settings_section(ui, theme, "CRC Computation", |ui| {
            ui.label(
                egui::RichText::new("When to compute CRC checksums for encrypted files")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
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
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                let move_btn = egui::Button::new("Move vault…").min_size(egui::vec2(120.0, 32.0));
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

                let rekey_btn = egui::Button::new("Rekey vault…").min_size(egui::vec2(120.0, 32.0));
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
    });

    action
}
