//! Archives Settings Page
//!
//! Contains settings for extraction, compression, and integrity verification.

use crate::features::settings::types::{
    ArchivesSettingsState, ChecksumAlgorithm, ChecksumMode, SettingsAction,
};
use crate::shared::theme::AppTheme;
use eframe::egui;

use super::render_settings_section;

/// Render the Archives settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ArchivesSettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Section: Extraction
        render_settings_section(ui, theme, "Extraction", |ui| {
            ui.label(
                egui::RichText::new("Configure how files are extracted from archives")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.label(
                egui::RichText::new("Directory used for intermediate operations (like conversion)")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(8.0);

            ui.label(
                egui::RichText::new("Temporary Directory")
                    .size(12.0)
                    .strong()
                    .color(theme.colors.on_surface),
            );
            ui.add_space(4.0);

            let default_temp = std::env::temp_dir();
            let default_temp_str = default_temp.to_string_lossy();

            ui.horizontal(|ui| {
                let te = egui::TextEdit::singleline(&mut state.temp_dir).hint_text(default_temp_str.as_ref());
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
            if state.temp_dir.trim().is_empty() {
                ui.label(
                    egui::RichText::new(format!("Default: System Temporary Directory ({})", default_temp_str))
                        .size(11.0)
                        .color(theme.colors.on_surface_variant)
                        .italics(),
                );
            }
        });

        ui.add_space(8.0);

        // Section: Compression
        render_settings_section(ui, theme, "Compression", |ui| {
            ui.label(
                egui::RichText::new("Settings for creating and modifying archives")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
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
                .color(theme.colors.on_surface_variant),
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
                            .color(theme.colors.on_surface),
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
                            .color(theme.colors.on_surface),
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

        // Section: Cache Management
        render_settings_section(ui, theme, "Cache Management", |ui| {
            ui.label(
                egui::RichText::new("Manage the application cache (thumbnails, metadata, etc.)")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
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
                egui::RichText::new(
                    "Clearing index removes database entries. Clearing content removes files from disk.",
                )
                .size(10.0)
                .italics()
                .color(theme.colors.on_surface_variant),
            );
        });
    });

    action
}
