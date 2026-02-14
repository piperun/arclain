//! Merge Dialog Component
//!
//! Dialog for merging multi-part archives into a single archive.

use super::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use arclain_core::archive::MultiPartArchive;
use arclain_core::services::{CompressionLevel, OutputFormat};
use arclain_widgets::{ButtonSize, TextButton, ThemedDropdown};
use eframe::egui;
use std::cell::Cell;
use std::path::PathBuf;

/// State for the merge dialog
#[derive(Debug, Clone, PartialEq)]
pub struct MergeDialogState {
    pub show: bool,
    pub multipart: Option<MultiPartArchive>,
    pub output_format: OutputFormat,
    pub compression_level: CompressionLevel,
    pub output_path: Option<PathBuf>,
    pub delete_originals: bool,
    pub password: String,
    pub error: Option<String>,
}

impl Default for MergeDialogState {
    fn default() -> Self {
        Self {
            show: false,
            multipart: None,
            output_format: OutputFormat::SevenZip,
            compression_level: CompressionLevel::Normal,
            output_path: None,
            delete_originals: false,
            password: String::new(),
            error: None,
        }
    }
}

impl MergeDialogState {
    /// Open the dialog with a detected multi-part archive
    pub fn open(&mut self, multipart: MultiPartArchive) {
        self.multipart = Some(multipart);
        self.show = true;
        self.error = None;
        self.output_path = None;
        self.delete_originals = false;
        self.password.clear();
    }

    /// Close the dialog
    pub fn close(&mut self) {
        self.show = false;
        self.multipart = None;
    }

    /// Get a preview of the output filename
    pub fn preview_output_name(&self) -> Option<String> {
        self.multipart.as_ref().map(|mp| {
            format!("{}.{}", mp.base_name, self.output_format.extension())
        })
    }
}

/// Result from the merge dialog
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MergeDialogResult {
    None,
    StartMerge,
    Cancel,
}

/// Render the merge dialog
pub fn render_merge_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &mut MergeDialogState,
) -> MergeDialogResult {
    if !state.show {
        return MergeDialogResult::None;
    }

    let params = ModalParams {
        width_frac: 0.4,
        height_frac: 0.5,
        min: egui::vec2(450.0, 400.0),
        max: egui::vec2(600.0, 500.0),
        padding: egui::vec2(20.0, 16.0),
        bottom_bar_height: 60.0,
        ..Default::default()
    };

    // Use Cell to share result between closures
    let result = Cell::new(MergeDialogResult::None);
    let can_merge = state.multipart.is_some();

    show_dimmed_modal(
        ctx,
        theme,
        "merge_dialog",
        &params,
        |ui, _rect| {
            // Title
            ui.label(
                egui::RichText::new("Merge Split Archive")
                    .size(18.0)
                    .strong()
                    .color(theme.colors.on_surface),
            );
            ui.add_space(16.0);

            if let Some(ref multipart) = state.multipart {
                // Archive info section
                ui.group(|ui| {
                    ui.label(
                        egui::RichText::new("Detected Archive")
                            .size(14.0)
                            .strong()
                            .color(theme.colors.on_surface),
                    );
                    ui.add_space(8.0);

                    egui::Grid::new("merge_info_grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Base Name:")
                                    .color(theme.colors.on_surface_variant),
                            );
                            ui.label(
                                egui::RichText::new(&multipart.base_name)
                                    .color(theme.colors.on_surface),
                            );
                            ui.end_row();

                            ui.label(
                                egui::RichText::new("Format:")
                                    .color(theme.colors.on_surface_variant),
                            );
                            ui.label(
                                egui::RichText::new(multipart.format.description())
                                    .color(theme.colors.on_surface),
                            );
                            ui.end_row();

                            ui.label(
                                egui::RichText::new("Parts Found:")
                                    .color(theme.colors.on_surface_variant),
                            );
                            ui.label(
                                egui::RichText::new(format!("{}", multipart.all_parts.len()))
                                    .color(theme.colors.on_surface),
                            );
                            ui.end_row();
                        });
                });

                ui.add_space(16.0);

                // Output options section
                ui.group(|ui| {
                    ui.label(
                        egui::RichText::new("Output Options")
                            .size(14.0)
                            .strong()
                            .color(theme.colors.on_surface),
                    );
                    ui.add_space(8.0);

                    egui::Grid::new("merge_options_grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            // Output format
                            ui.label(
                                egui::RichText::new("Output Format:")
                                    .color(theme.colors.on_surface_variant),
                            );
                            ThemedDropdown::new("output_format", state.output_format.display_name())
                                .with_theme_colors(&theme.colors)
                                .show_ui(ui, |ui| {
                                    for format in OutputFormat::all() {
                                        ui.selectable_value(
                                            &mut state.output_format,
                                            *format,
                                            format.display_name(),
                                        );
                                    }
                                });
                            ui.end_row();

                            // Compression level
                            ui.label(
                                egui::RichText::new("Compression:")
                                    .color(theme.colors.on_surface_variant),
                            );
                            ThemedDropdown::new("compression_level", state.compression_level.display_name())
                                .with_theme_colors(&theme.colors)
                                .show_ui(ui, |ui| {
                                    for level in CompressionLevel::all() {
                                        ui.selectable_value(
                                            &mut state.compression_level,
                                            *level,
                                            level.display_name(),
                                        );
                                    }
                                });
                            ui.end_row();
                        });

                    ui.add_space(8.0);

                    // Delete originals checkbox
                    ui.checkbox(
                        &mut state.delete_originals,
                        egui::RichText::new("Delete original parts after merge")
                            .color(theme.colors.on_surface),
                    );

                    ui.add_space(8.0);

                    // Output preview
                    if let Some(output_name) = state.preview_output_name() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Output:")
                                    .color(theme.colors.on_surface_variant),
                            );
                            ui.label(
                                egui::RichText::new(&output_name)
                                    .color(theme.colors.primary)
                                    .strong(),
                            );
                        });
                    }
                });

                // Error display
                if let Some(ref error) = state.error {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(error)
                            .color(theme.colors.error)
                            .size(13.0),
                    );
                }
            } else {
                ui.label(
                    egui::RichText::new("No multi-part archive detected")
                        .color(theme.colors.on_surface_variant),
                );
            }
        },
        |ui| {
            // Bottom button bar
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        can_merge,
                        TextButton::new("Merge", ButtonSize::Medium)
                            .variant(arclain_theme::ButtonVariant::Primary)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    result.set(MergeDialogResult::StartMerge);
                }

                ui.add_space(8.0);

                if ui
                    .add(
                        TextButton::new("Cancel", ButtonSize::Medium)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    result.set(MergeDialogResult::Cancel);
                }
            });
        },
    );

    // Handle Escape key
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        result.set(MergeDialogResult::Cancel);
    }

    let final_result = result.get();
    if final_result == MergeDialogResult::Cancel {
        state.close();
    }

    final_result
}
