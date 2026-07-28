//! Pure presentation for the file-edit dialog: renders `dialog`'s current
//! state and returns what the user did (`Save`/`Cancel`), touching no
//! persistence or facade of its own. `dialog.name_input`/`.content` are
//! plain egui text-edit targets; nothing here decides whether a save can
//! actually happen -- the `Save` button is enabled purely on
//! `FileEditLoadState::Ready` (the content finished loading), the same
//! way the toolbar's own Add/Delete buttons enable on selection state
//! alone rather than a backend capability check.
//!
//! The actual save wiring lives in
//! `crate::core::arclain_app::dialog_handler`'s `FileEditResult::Save`
//! handler, which submits `dialog.content` through
//! `crate::core::operations::file::start_replace_text` -- the
//! application facade (`ArclainApp::start_archive_mutation` with
//! `ReplaceText`) is what actually checks
//! `BackendCapabilities::can_modify_files` and reports `Unsupported` if
//! the archive's backend cannot save at all; this dialog does not
//! pre-empt that with its own gating, matching the toolbar's existing
//! "attempt, then surface the resulting error" convention rather than
//! introducing a second, inconsistent one just for this dialog.

use crate::features::file_editing::domain::types::{
    FileEditDialog, FileEditLoadState, FileEditResult,
};
use crate::shared::dialogs::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use arclain_theme::ButtonVariant;
use arclain_widgets::{ButtonSize, TextButton, TextInput};
use eframe::egui;

pub fn render_file_edit_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut FileEditDialog,
) -> Option<FileEditResult> {
    if !dialog.show {
        return None;
    }
    let mut result = None;
    let load_state = dialog.load_state.clone();
    let can_save = matches!(load_state, FileEditLoadState::Ready);

    let params = ModalParams {
        width_frac: 0.6,
        height_frac: 0.7,
        min: egui::vec2(520.0, 420.0),
        max: egui::vec2(900.0, 900.0),
        padding: egui::vec2(20.0, 16.0),
        bottom_bar_height: 56.0,
        overlay_alpha: 180,
        overlay_order: egui::Order::Middle,
        modal_order: egui::Order::Foreground,
    };

    // Click flags to avoid borrowing dialog in both closures
    let mut want_save = false;
    let mut want_cancel = false;

    show_dimmed_modal(
        ctx,
        theme,
        "file_edit",
        &params,
        |ui, content_rect| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("✏ Edit File").size(18.0).strong());
                ui.label(
                    egui::RichText::new("— inline editor")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );
            });

            match &load_state {
                FileEditLoadState::Ready => {
                    ui.label(
                        egui::RichText::new("File name")
                            .size(12.0)
                            .color(theme.colors.on_surface_variant),
                    );
                    TextInput::new(&mut dialog.name_input)
                        .width(content_rect.width())
                        .with_theme_colors(&theme.colors)
                        .show(ui);

                    ui.label(
                        egui::RichText::new("Content")
                            .size(12.0)
                            .color(theme.colors.on_surface_variant),
                    );
                    ui.add_sized(
                        [content_rect.width(), content_rect.height() - 140.0],
                        egui::TextEdit::multiline(&mut dialog.content)
                            .font(egui::TextStyle::Monospace)
                            .code_editor(),
                    );

                    if !dialog.error.is_empty() {
                        crate::shared::components::error_label(ui, theme, &dialog.error);
                    }
                }
                FileEditLoadState::Loading { .. } => {
                    ui.add_space(48.0);
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new("Loading file content…").strong());
                        ui.label(
                            egui::RichText::new(&dialog.full_path_in_archive)
                                .size(11.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    });
                }
                FileEditLoadState::Failed(error) => {
                    ui.add_space(48.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Unable to load file")
                                .strong()
                                .color(theme.colors.error),
                        );
                        crate::shared::components::error_label(ui, theme, error);
                    });
                }
                FileEditLoadState::Idle => {
                    ui.add_space(48.0);
                    ui.vertical_centered(|ui| {
                        ui.label("File content is not ready");
                    });
                }
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let dialog_btn_size = ButtonSize::Custom {
                    width: 100.0,
                    height: 32.0,
                };
                if ui
                    .add_enabled(
                        can_save,
                        TextButton::new("Save", dialog_btn_size)
                            .variant(ButtonVariant::Primary)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    want_save = true;
                }
                if ui
                    .add(
                        TextButton::new("Cancel", dialog_btn_size)
                            .variant(ButtonVariant::Secondary)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    want_cancel = true;
                }
            });
        },
    );

    if want_save && can_save {
        result = Some(FileEditResult::Save {
            new_name: dialog.name_input.clone(),
            content: dialog.content.clone(),
        });
    }
    if want_cancel {
        result = Some(FileEditResult::Cancel);
    }

    result
}
