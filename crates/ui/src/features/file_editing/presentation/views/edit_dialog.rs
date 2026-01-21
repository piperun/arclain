use crate::features::file_editing::domain::types::{FileEditDialog, FileEditResult};
use crate::shared::dialogs::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
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

            ui.label(
                egui::RichText::new("File name")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_sized(
                [content_rect.width(), 32.0],
                egui::TextEdit::singleline(&mut dialog.name_input),
            );

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
                ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error);
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = ui.add(
                    egui::Button::new(egui::RichText::new("Save").strong())
                        .min_size(egui::vec2(100.0, 32.0)),
                );
                let cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 32.0)));
                if save.clicked() {
                    want_save = true;
                }
                if cancel.clicked() {
                    want_cancel = true;
                }
            });
        },
    );

    if want_save {
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
