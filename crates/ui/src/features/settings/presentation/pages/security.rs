//! Security Settings Page
//!
//! Contains settings for encryption, CRC policy, and vault management.

use arclain_widgets::{ButtonSize, TextButton, TextInput, TextInputSize, ThemedDropdown};
use crate::features::settings::types::{EncryptedCrcPolicy, SecuritySettingsState, SettingsAction};
use crate::shared::components::settings_form::{Form, SettingsGroup};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the Security settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut SecuritySettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    Form::new().show(ui, theme, |ui| {
        SettingsGroup::new("Encryption")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Master key and secrets database configuration")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Calculate default paths for hints
                let defaults =
                    arclain_core::config::database::DbPaths::calculate_defaults("arclain").ok();
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
                        .color(colors.on_surface),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let hint = default_key.as_deref().unwrap_or("Select a key file...");
                    let mut binding = state.key_file_path.write();
                    TextInput::new(&mut *binding)
                        .hint(hint)
                        .size(TextInputSize::Small)
                        .width(ui.available_width() - 110.0)
                        .with_theme_colors(colors)
                        .show(ui);
                    if ui
                        .add(
                            TextButton::new("Browse…", ButtonSize::custom(100.0, 28.0))
                                .with_theme_colors(colors),
                        )
                        .clicked()
                    {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            *state.key_file_path.write() = file.to_string_lossy().to_string();
                        }
                    }
                });
                if state.key_file_path.read().trim().is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "Default: System AppData location ({})",
                            default_key.as_deref().unwrap_or("Unknown")
                        ))
                        .size(11.0)
                        .color(colors.on_surface_variant)
                        .italics(),
                    );
                }

                ui.add_space(12.0);

                // Secrets DB picker
                ui.label(
                    egui::RichText::new("Secrets database (redb with AES-256-GCM)")
                        .size(12.0)
                        .strong()
                        .color(colors.on_surface),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let hint = default_db.as_deref().unwrap_or("Path to pass.redb...");
                    let mut binding = state.secrets_db_path.write();
                    TextInput::new(&mut *binding)
                        .hint(hint)
                        .size(TextInputSize::Small)
                        .width(ui.available_width() - 110.0)
                        .with_theme_colors(colors)
                        .show(ui);
                    if ui
                        .add(
                            TextButton::new("Browse…", ButtonSize::custom(100.0, 28.0))
                                .with_theme_colors(colors),
                        )
                        .clicked()
                    {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            *state.secrets_db_path.write() = file.to_string_lossy().to_string();
                        }
                    }
                });
                if state.secrets_db_path.read().trim().is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "Default: System AppData location ({})",
                            default_db.as_deref().unwrap_or("Unknown")
                        ))
                        .size(11.0)
                        .color(colors.on_surface_variant)
                        .italics(),
                    );
                }
            })
            .show(ui, &theme.colors);

        // Section: CRC Policy
        SettingsGroup::new("CRC Computation")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("When to compute CRC checksums for encrypted files")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(8.0);

                ThemedDropdown::new("crc_policy", state.encrypted_crc_policy.read().display_name())
                    .with_theme_colors(colors)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut *state.encrypted_crc_policy.write(),
                            EncryptedCrcPolicy::OnOpen,
                            EncryptedCrcPolicy::OnOpen.display_name(),
                        );
                        ui.selectable_value(
                            &mut *state.encrypted_crc_policy.write(),
                            EncryptedCrcPolicy::PromptOnOpen,
                            EncryptedCrcPolicy::PromptOnOpen.display_name(),
                        );
                        ui.selectable_value(
                            &mut *state.encrypted_crc_policy.write(),
                            EncryptedCrcPolicy::OnAccess,
                            EncryptedCrcPolicy::OnAccess.display_name(),
                        );
                    });
            })
            .show(ui, &theme.colors);

        // Section: Vault Management
        SettingsGroup::new("Vault Management")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Move or rekey your encrypted secrets vault")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    if ui
                        .add(
                            TextButton::new("Move vault…", ButtonSize::Large)
                                .with_theme_colors(colors),
                        )
                        .clicked()
                    {
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

                    if ui
                        .add(
                            TextButton::new("Rekey vault…", ButtonSize::Large)
                                .with_theme_colors(colors),
                        )
                        .clicked()
                    {
                        if let Some(file) = rfd::FileDialog::new().pick_file() {
                            action = Some(SettingsAction::RekeyVault {
                                new_key_file_path: file.to_string_lossy().to_string(),
                            });
                        }
                    }
                });
            })
            .show(ui, &theme.colors);

        // Info / Error messages using theme colors
        if !state.info.read().is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(&*state.info.read())
                    .color(theme.colors.success),
            );
        }
        if !state.error.read().is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(&*state.error.read())
                    .color(theme.colors.error),
            );
        }
    });

    action
}
