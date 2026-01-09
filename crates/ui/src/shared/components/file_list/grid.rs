//! Grid view for file list

use super::types::{FileEntry, FileListAction};
use crate::shared::theme::AppTheme;
use arclain_widgets::pixel_align;
use eframe::egui;

/// Render files in a grid layout
pub fn render_grid_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &mut [FileEntry],
) -> Option<FileListAction> {
    let mut action: Option<FileListAction> = None;
    let available_width = ui.available_width();
    let item_width = 280.0;
    let columns = (available_width / item_width).floor().max(1.0) as usize;

    ui.spacing_mut().item_spacing = egui::vec2(1.0, 1.0);

    egui::Grid::new("file_grid")
        .num_columns(columns)
        .spacing([1.0, 1.0])
        .show(ui, |ui| {
            for idx in 0..entries.len() {
                if idx > 0 && idx % columns == 0 {
                    ui.end_row();
                }

                let (response, row_action) = render_grid_item(ui, theme, &mut entries[idx]);

                if response.clicked() {
                    entries[idx].selected = !entries[idx].selected;
                }

                if response.double_clicked() {
                    if entries[idx].is_folder {
                        action = Some(FileListAction::Navigate(entries[idx].name.clone()));
                    } else {
                        action = Some(FileListAction::Open(entries[idx].name.clone()));
                    }
                }

                if action.is_none() {
                    if let Some(a) = row_action {
                        action = Some(a);
                    }
                }
            }
        });

    action
}

fn render_grid_item(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entry: &mut FileEntry,
) -> (egui::Response, Option<FileListAction>) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(280.0, 80.0), egui::Sense::click());
    let mut action: Option<FileListAction> = None;

    if ui.is_rect_visible(rect) {
        // Background
        let bg_color = if entry.selected {
            theme.colors.selection
        } else if response.hovered() {
            theme.colors.surface_variant
        } else {
            theme.colors.surface
        };
        ui.painter().rect_filled(rect, 0.0, bg_color);

        // Content
        let content_rect = rect.shrink2(egui::vec2(12.0, 8.0));

        // Icon
        let icon_size = 32.0;
        let icon_rect =
            egui::Rect::from_min_size(content_rect.min, egui::vec2(icon_size, icon_size));
        ui.painter()
            .rect_filled(icon_rect, 4.0, theme.colors.surface_variant);

        let ext = entry
            .name
            .split('.')
            .next_back()
            .unwrap_or("")
            .to_uppercase();
        let ext_text: &str = if entry.is_folder { "📁" } else { &ext };

        ui.painter().text(
            pixel_align(icon_rect.center()),
            egui::Align2::CENTER_CENTER,
            ext_text,
            egui::FontId::proportional(12.0),
            theme.colors.on_surface_variant,
        );

        // File info
        let text_x = content_rect.min.x + icon_size + 12.0;
        let name_pos = pixel_align(egui::pos2(text_x, content_rect.min.y + 4.0));
        let meta_pos = pixel_align(egui::pos2(text_x, content_rect.min.y + 24.0));

        let text_color = theme.colors.on_surface;
        let meta_color = if entry.selected {
            theme.colors.on_surface
        } else {
            theme.colors.on_surface_variant
        };

        ui.painter().text(
            name_pos,
            egui::Align2::LEFT_TOP,
            &entry.name,
            egui::FontId::proportional(14.0),
            text_color,
        );
        ui.painter().text(
            meta_pos,
            egui::Align2::LEFT_TOP,
            format!("{} • {}", entry.size, entry.modified),
            egui::FontId::proportional(12.0),
            meta_color,
        );

        // Inline actions (Edit/Delete) aligned to the right
        let actions_w = 60.0;
        let actions_h = 24.0;
        let actions_rect = egui::Rect::from_min_size(
            egui::pos2(content_rect.max.x - actions_w, content_rect.min.y),
            egui::vec2(actions_w, actions_h),
        );

        ui.scope_builder(egui::UiBuilder::new().max_rect(actions_rect), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                // Edit: only for files
                let can_edit = !entry.is_folder;
                let edit_clicked = ui
                    .add_enabled(
                        can_edit,
                        egui::Button::new("✏").min_size(egui::vec2(26.0, 22.0)),
                    )
                    .on_hover_text(if can_edit {
                        "Edit file"
                    } else {
                        "Cannot edit folders"
                    })
                    .clicked();
                if edit_clicked {
                    action = Some(FileListAction::Edit(entry.name.clone()));
                }

                let del_clicked = ui
                    .add_sized(egui::vec2(26.0, 22.0), egui::Button::new("🗑"))
                    .on_hover_text("Delete")
                    .clicked();
                if del_clicked {
                    action = Some(FileListAction::Delete(entry.name.clone()));
                }
            });
        });
    }

    (response, action)
}
