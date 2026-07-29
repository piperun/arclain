//! Archives Settings Page
//!
//! Contains settings for extraction, compression, and integrity verification.

use crate::features::settings::types::{
    ArchivesSettingsState, ChecksumAlgorithm, ChecksumMode, CollisionPolicy, SettingsAction,
};
use crate::shared::components::settings_form::{Form, SettingsGroup, SettingsRow};
use crate::shared::theme::AppTheme;
use arclain_widgets::{
    ButtonSize, TextButton, TextInput, TextInputSize, ThemedDropdown, ToggleSwitch,
};
use eframe::egui;

/// Render the Archives settings page.
///
/// `stored_collision_policy` is the token the application currently has
/// stored; it seeds the dropdown once and is ignored on later frames so
/// a selection in progress is never overwritten mid-interaction.
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ArchivesSettingsState,
    stored_collision_policy: &str,
) -> Option<SettingsAction> {
    let mut action = None;

    if !*state.collision_policy_loaded.read() {
        *state.default_collision_policy.write() =
            CollisionPolicy::from_settings_str(stored_collision_policy);
        *state.collision_policy_loaded.write() = true;
    }

    Form::new().show(ui, theme, |ui| {
        // Section: Extraction
        SettingsGroup::new("Extraction")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Configure how files are extracted from archives")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.label(
                    egui::RichText::new("Directory used for intermediate operations (like conversion)")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(8.0);

                ui.label(
                    egui::RichText::new("Temporary Directory")
                        .size(12.0)
                        .strong()
                        .color(colors.on_surface),
                );
                ui.add_space(4.0);

                let default_temp = std::env::temp_dir();
                let default_temp_str = default_temp.to_string_lossy();

                ui.horizontal(|ui| {
                    let mut binding = state.temp_dir.write();
                    TextInput::new(&mut *binding)
                        .hint(&*default_temp_str)
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
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            *state.temp_dir.write() = path.to_string_lossy().into_owned();
                        }
                    }
                });
                if state.temp_dir.read().trim().is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "Default: System Temporary Directory ({})",
                            default_temp_str
                        ))
                        .size(11.0)
                        .color(colors.on_surface_variant)
                        .italics(),
                    );
                }
            })
            .show(ui, &theme.colors);

        // Section: Pipeline
        SettingsGroup::new("Pipeline")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new(
                        "Default behavior when a batch pipeline's output path already exists. \
                         Individual pipelines can override this via the Process page."
                    )
                    .size(12.0)
                    .color(colors.on_surface_variant),
                );
                ui.add_space(8.0);

                ui.label(
                    egui::RichText::new("If output exists")
                        .size(12.0)
                        .strong()
                        .color(colors.on_surface),
                );
                ui.add_space(4.0);

                let current = *state.default_collision_policy.read();
                let mut next = current;
                ThemedDropdown::new("settings_default_collision_policy", current.display_name())
                    .with_theme_colors(colors)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for opt in CollisionPolicy::ALL {
                            if ui
                                .selectable_label(current == opt, opt.display_name())
                                .clicked()
                            {
                                next = opt;
                            }
                        }
                    });

                if next != current {
                    *state.default_collision_policy.write() = next;
                    action = Some(SettingsAction::SaveCollisionPolicy { policy: next });
                }
            })
            .show(ui, &theme.colors);

        // Section: Integrity Verification
        SettingsGroup::new("Integrity Verification")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new(
                        "Verify file integrity after extraction and organization operations",
                    )
                    .size(12.0)
                    .color(colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Enable toggle using SettingsRow
                SettingsRow::new("Enable integrity verification")
                    .description("Compute checksums to verify file integrity")
                    .action(|ui| {
                        ui.add(
                            ToggleSwitch::new(&mut *state.checksum_enabled.write())
                                .with_theme_colors(colors),
                        );
                    })
                    .show(ui, colors);

                // Only show options if enabled
                if *state.checksum_enabled.read() {
                    ui.add_space(8.0);

                    // Mode selector
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Verification Mode:")
                                .size(12.0)
                                .color(colors.on_surface),
                        );
                        ThemedDropdown::new("checksum_mode", state.checksum_mode.read().display_name())
                            .with_theme_colors(colors)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut *state.checksum_mode.write(),
                                    ChecksumMode::Simple,
                                    ChecksumMode::Simple.display_name(),
                                );
                                ui.selectable_value(
                                    &mut *state.checksum_mode.write(),
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
                                .color(colors.on_surface),
                        );
                        ThemedDropdown::new("checksum_algorithm", state.checksum_algorithm.read().display_name())
                            .with_theme_colors(colors)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut *state.checksum_algorithm.write(),
                                    ChecksumAlgorithm::Crc32,
                                    ChecksumAlgorithm::Crc32.display_name(),
                                );
                                ui.selectable_value(
                                    &mut *state.checksum_algorithm.write(),
                                    ChecksumAlgorithm::XxHash,
                                    ChecksumAlgorithm::XxHash.display_name(),
                                );
                                ui.selectable_value(
                                    &mut *state.checksum_algorithm.write(),
                                    ChecksumAlgorithm::Sha256,
                                    ChecksumAlgorithm::Sha256.display_name(),
                                );
                            });
                    });
                    ui.add_space(8.0);

                    // Verification triggers using SettingsRow
                    SettingsRow::new("Verify after extraction")
                        .description("Check file integrity after extracting from archives")
                        .action(|ui| {
                            ui.add(
                                ToggleSwitch::new(&mut *state.verify_after_extract.write())
                                    .with_theme_colors(colors),
                            );
                        })
                        .show(ui, colors);

                    SettingsRow::new("Verify after organize")
                        .description("Check file integrity after organization operations")
                        .action(|ui| {
                            ui.add(
                                ToggleSwitch::new(&mut *state.verify_after_organize.write())
                                    .with_theme_colors(colors),
                            );
                        })
                        .show(ui, colors);
                }
            })
            .show(ui, &theme.colors);

        // Section: Cache Management
        SettingsGroup::new("Cache Management")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Manage the application cache (thumbnails, metadata, etc.)")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if ui
                        .add(
                            TextButton::new("Clear Cache Index", ButtonSize::Medium)
                                .with_theme_colors(colors),
                        )
                        .clicked()
                    {
                        action = Some(SettingsAction::ClearCacheIndex);
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            TextButton::new("Clear Cache Content", ButtonSize::Medium)
                                .with_theme_colors(colors),
                        )
                        .clicked()
                    {
                        action = Some(SettingsAction::ClearCacheContent);
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Clearing index removes database entries. Clearing content removes files from disk.",
                    )
                    .size(10.0)
                    .italics()
                    .color(colors.on_surface_variant),
                );
            })
            .show(ui, &theme.colors);

        // Section: Cache Maintenance
        SettingsGroup::new("Cache Maintenance")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Clean up orphaned data and fix cache entries")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if ui
                        .add(
                            TextButton::new("Garbage Collect", ButtonSize::Medium)
                                .with_theme_colors(colors),
                        )
                        .on_hover_text("Remove cache entries for deleted products")
                        .clicked()
                    {
                        action = Some(SettingsAction::GarbageCollectCache);
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            TextButton::new("Clean Search Cache", ButtonSize::Medium)
                                .with_theme_colors(colors),
                        )
                        .on_hover_text("Remove search results older than 7 days")
                        .clicked()
                    {
                        action = Some(SettingsAction::CleanOldSearchCache);
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            TextButton::new("Fix Cache Entries", ButtonSize::Medium)
                                .with_theme_colors(colors),
                        )
                        .on_hover_text("Update cache_type and product_id based on key patterns")
                        .clicked()
                    {
                        action = Some(SettingsAction::MigrateCacheEntries);
                    }
                });
            })
            .show(ui, &theme.colors);
    });

    action
}
