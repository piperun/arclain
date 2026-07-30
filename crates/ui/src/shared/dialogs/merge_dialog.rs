//! Merge Dialog Component
//!
//! Dialog for merging multi-part archives into a single archive.
//!
//! Reads the application facade's own split-archive vocabulary
//! (`arclain_app::archive::MultiPartArchiveDto`,
//! `arclain_app::operations::{MergeOutputFormat, MergeCompressionLevel}`)
//! rather than `arclain_core`'s. The merge itself is
//! `ArclainApp::start_merge`, dispatched by
//! `crate::core::arclain_app::dialog_handler` on
//! [`MergeDialogResult::StartMerge`] and rendered through the per-tab
//! progress dialog by `crate::core::operation_bridge`.

use super::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use arclain_app::archive::MultiPartArchiveDto;
use arclain_app::operations::{MergeCompressionLevel, MergeOutputFormat};
use arclain_widgets::{ButtonSize, TextButton, ThemedDropdown};
use eframe::egui;
use std::cell::Cell;

/// State for the merge dialog.
///
/// Three fields the pre-facade state carried are gone, all of them dead:
///
/// - `password` -- no widget here ever wrote to it, so it was always
///   empty and an encrypted set simply failed. The facade now raises its
///   own `Challenge::Password`, answered through the shared per-tab
///   password dialog every other operation already uses.
/// - `output_path` -- always `None`; the merge writes beside the set's
///   first part and nothing here ever offered to choose otherwise.
/// - `error` -- only ever cleared and rendered, never assigned. Both the
///   pre-facade path and this one report merge failures through the status
///   bar (now via `crate::core::operation_bridge`'s terminal handler), so
///   the in-dialog error line was unreachable. The dialog is also closed
///   by the time a merge can fail, which is why the status bar is the
///   right place for it.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeDialogState {
    pub show: bool,
    pub multipart: Option<MultiPartArchiveDto>,
    pub output_format: MergeOutputFormat,
    pub compression_level: MergeCompressionLevel,
    pub delete_originals: bool,
}

impl Default for MergeDialogState {
    fn default() -> Self {
        Self {
            show: false,
            multipart: None,
            output_format: MergeOutputFormat::SevenZip,
            compression_level: MergeCompressionLevel::Normal,
            delete_originals: false,
        }
    }
}

impl MergeDialogState {
    /// Open the dialog with a detected multi-part archive
    pub fn open(&mut self, multipart: MultiPartArchiveDto) {
        self.multipart = Some(multipart);
        self.show = true;
        self.delete_originals = false;
    }

    /// Close the dialog
    pub fn close(&mut self) {
        self.show = false;
        self.multipart = None;
    }

    /// Get a preview of the output filename
    pub fn preview_output_name(&self) -> Option<String> {
        self.multipart
            .as_ref()
            .map(|mp| format!("{}.{}", mp.base_name, self.output_format.extension()))
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
                                egui::RichText::new(format!("{}", multipart.parts.len()))
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
                            ThemedDropdown::new(
                                "output_format",
                                state.output_format.display_name(),
                            )
                            .with_theme_colors(&theme.colors)
                            .show_ui(ui, |ui| {
                                for format in MergeOutputFormat::all() {
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
                            ThemedDropdown::new(
                                "compression_level",
                                state.compression_level.display_name(),
                            )
                            .with_theme_colors(&theme.colors)
                            .show_ui(ui, |ui| {
                                for level in MergeCompressionLevel::all() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn detected(base_name: &str, parts: usize) -> MultiPartArchiveDto {
        MultiPartArchiveDto {
            first_part: PathBuf::from(format!("/sets/{base_name}.part1.rar")),
            base_name: base_name.to_string(),
            format: arclain_app::archive::MultiPartFormat::RarPart,
            parts: (1..=parts)
                .map(|index| PathBuf::from(format!("/sets/{base_name}.part{index}.rar")))
                .collect(),
        }
    }

    #[test]
    fn opening_shows_the_set_and_resets_the_destructive_opt_in() {
        let mut state = MergeDialogState {
            delete_originals: true,
            ..Default::default()
        };
        state.open(detected("rj123456", 3));
        assert!(state.show);
        assert_eq!(
            state.multipart.as_ref().map(|mp| mp.parts.len()),
            Some(3),
            "the dialog reports the parts detection actually found"
        );
        assert!(
            !state.delete_originals,
            "a fresh open must not inherit the previous run's destructive opt-in"
        );
    }

    #[test]
    fn closing_forgets_the_set() {
        let mut state = MergeDialogState::default();
        state.open(detected("rj123456", 2));
        state.close();
        assert!(!state.show);
        assert!(state.multipart.is_none());
    }

    #[test]
    fn the_output_preview_follows_the_selected_format() {
        let mut state = MergeDialogState::default();
        assert_eq!(state.preview_output_name(), None);

        state.open(detected("rj123456", 2));
        assert_eq!(
            state.preview_output_name().as_deref(),
            Some("rj123456.7z"),
            "the default format is 7z"
        );

        state.output_format = MergeOutputFormat::Zip;
        assert_eq!(state.preview_output_name().as_deref(), Some("rj123456.zip"));
    }
}
