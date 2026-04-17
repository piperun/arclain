//! Archive conversion options dialog.
//!
//! Lets the user pick target format, compression level, password, and
//! whether to flatten nested archives before repacking.

use crate::core::signals::ConvertDialogState;
use crate::shared::dialogs::helpers::{show_dimmed_modal, ModalParams};
use arclain_core::{CompressionLevel, ConvertFormat};
use arclain_theme::AppTheme;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;

/// Render the conversion options dialog. Mutates `state` in place.
/// When the user clicks Convert, sets `state.should_start = true` and closes.
pub fn render(ctx: &egui::Context, theme: &AppTheme, state: &mut ConvertDialogState) {
    if !state.show {
        return;
    }

    let params = ModalParams {
        width_frac: 0.45,
        height_frac: 0.55,
        min: egui::vec2(420.0, 400.0),
        max: egui::vec2(620.0, 520.0),
        bottom_bar_height: 48.0,
        ..Default::default()
    };

    show_dimmed_modal(
        ctx,
        theme,
        "convert_dialog",
        &params,
        |ui, _rect| {
            ui.heading("Convert Archive");
            ui.add_space(8.0);

            // Format picker
            ui.label("Format:");
            egui::ComboBox::from_id_salt("convert_format")
                .selected_text(format!(".{}", state.options.format.extension()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.options.format, ConvertFormat::Zip, ".zip");
                    ui.selectable_value(
                        &mut state.options.format,
                        ConvertFormat::SevenZ,
                        ".7z",
                    );
                });

            ui.add_space(8.0);

            // Compression
            ui.label("Compression:");
            egui::ComboBox::from_id_salt("convert_compression")
                .selected_text(match state.options.compression {
                    CompressionLevel::Fast => "Fast",
                    CompressionLevel::Normal => "Normal",
                    CompressionLevel::Max => "Max",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut state.options.compression,
                        CompressionLevel::Fast,
                        "Fast",
                    );
                    ui.selectable_value(
                        &mut state.options.compression,
                        CompressionLevel::Normal,
                        "Normal",
                    );
                    ui.selectable_value(
                        &mut state.options.compression,
                        CompressionLevel::Max,
                        "Max",
                    );
                });

            ui.add_space(8.0);

            // Password
            ui.label("Password (optional):");
            let mut pw = state.options.password.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut pw).password(true))
                .changed()
            {
                state.options.password = if pw.is_empty() { None } else { Some(pw) };
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            ui.checkbox(&mut state.options.flatten_nested, "Flatten nested archives");
            ui.label(
                egui::RichText::new(
                    "Extracts inner archives (.rar, .zip, .7z) as sibling folders.",
                )
                .size(11.0)
                .color(theme.colors.on_surface_variant),
            );

            ui.add_space(6.0);

            ui.add_enabled_ui(state.options.flatten_nested, |ui| {
                ui.checkbox(
                    &mut state.options.strip_common_prefix,
                    "Strip common prefix from folder names",
                );
                ui.label(
                    egui::RichText::new(
                        "E.g. 'ModName - Main' → 'Main' when all variants share a prefix.",
                    )
                    .size(11.0)
                    .color(theme.colors.on_surface_variant),
                );
            });
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        TextButton::new("Convert", ButtonSize::Small)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    state.should_start = true;
                    state.show = false;
                }
                if ui
                    .add(
                        TextButton::new("Cancel", ButtonSize::Small)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    state.show = false;
                    state.should_start = false;
                }
            });
        },
    );
}
