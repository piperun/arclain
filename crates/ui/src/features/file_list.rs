use super::theme::AppTheme;
use eframe::egui;

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: String,
    pub compressed: String,
    pub ratio: String,
    pub modified: String,
    pub encrypted: bool,
    pub is_folder: bool,
    pub selected: bool,
}

pub fn render_breadcrumb(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    current_path: &str,
    archive_name: &str,
) -> Option<String> {
    let mut navigate_to: Option<String> = None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

        // Root archive button (clickable)
        let root_response = ui.add(
            egui::Label::new(
                egui::RichText::new(archive_name)
                    .size(14.0)
                    .color(theme.colors.text_primary),
            )
            .sense(egui::Sense::click()),
        );

        if root_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            // Draw underline on hover
            let rect = root_response.rect;
            ui.painter().line_segment(
                [
                    egui::pos2(rect.min.x, rect.max.y),
                    egui::pos2(rect.max.x, rect.max.y),
                ],
                egui::Stroke::new(1.0, theme.colors.text_primary),
            );
        }

        if root_response.clicked() {
            navigate_to = Some(String::new()); // Navigate to root
        }

        if !current_path.is_empty() {
            ui.label(
                egui::RichText::new("/")
                    .size(14.0)
                    .color(theme.colors.text_muted),
            );

            // Split path into segments and make each clickable
            let segments: Vec<&str> = current_path.split('/').collect();
            for (idx, segment) in segments.iter().enumerate() {
                if idx > 0 {
                    ui.label(
                        egui::RichText::new("/")
                            .size(14.0)
                            .color(theme.colors.text_muted),
                    );
                }

                let is_last = idx == segments.len() - 1;
                let text_color = if is_last {
                    theme.colors.text_primary
                } else {
                    theme.colors.text_secondary
                };

                let segment_response = ui.add(
                    egui::Label::new(egui::RichText::new(*segment).size(14.0).color(text_color))
                        .sense(egui::Sense::click()),
                );

                if segment_response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    // Draw underline on hover
                    let rect = segment_response.rect;
                    ui.painter().line_segment(
                        [
                            egui::pos2(rect.min.x, rect.max.y),
                            egui::pos2(rect.max.x, rect.max.y),
                        ],
                        egui::Stroke::new(1.0, text_color),
                    );
                }

                if segment_response.clicked() {
                    // Build path up to this segment
                    let target_path = segments[..=idx].join("/");
                    navigate_to = Some(target_path);
                }
            }
        }
    });

    navigate_to
}

pub fn render_grid_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &mut [FileEntry],
) -> Option<String> {
    let mut navigate_to: Option<String> = None;
    let available_width = ui.available_width();
    let item_width = 280.0;
    let columns = (available_width / item_width).floor().max(1.0) as usize;

    ui.spacing_mut().item_spacing = egui::vec2(1.0, 1.0);

    egui::Grid::new("file_grid")
        .num_columns(columns)
        .spacing([1.0, 1.0])
        .show(ui, |ui| {
            for (idx, entry) in entries.iter_mut().enumerate() {
                if idx > 0 && idx % columns == 0 {
                    ui.end_row();
                }

                let response = render_grid_item(ui, theme, entry);

                if response.clicked() {
                    entry.selected = !entry.selected;
                }

                if response.double_clicked() && entry.is_folder {
                    navigate_to = Some(entry.name.clone());
                }
            }
        });

    navigate_to
}

fn render_grid_item(ui: &mut egui::Ui, theme: &AppTheme, entry: &FileEntry) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(280.0, 60.0), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        // Background
        let bg_color = if entry.selected {
            theme.colors.selection
        } else if response.hovered() {
            theme.colors.bg_hover
        } else {
            theme.colors.bg_primary
        };

        ui.painter().rect_filled(rect, 0.0, bg_color);

        // Content
        let content_rect = rect.shrink2(egui::vec2(12.0, 8.0));

        // Icon
        let icon_size = 32.0;
        let icon_rect =
            egui::Rect::from_min_size(content_rect.min, egui::vec2(icon_size, icon_size));

        ui.painter()
            .rect_filled(icon_rect, 4.0, theme.colors.bg_tertiary);

        let ext = entry.name.split('.').last().unwrap_or("").to_uppercase();
        let ext_text = if entry.is_folder { "📁" } else { &ext };

        ui.painter().text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            ext_text,
            egui::FontId::proportional(12.0),
            theme.colors.text_muted,
        );

        // File info
        let text_x = content_rect.min.x + icon_size + 12.0;
        let name_pos = egui::pos2(text_x, content_rect.min.y + 8.0);
        let meta_pos = egui::pos2(text_x, content_rect.min.y + 28.0);

        // Use selection text color when selected
        let text_color = if entry.selected {
            theme.colors.selection_text
        } else {
            theme.colors.text_primary
        };

        let meta_color = if entry.selected {
            theme.colors.selection_text
        } else {
            theme.colors.text_muted
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
    }

    response
}

pub fn render_list_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &mut [FileEntry],
    columns_locked: bool,
) -> Option<String> {
    use egui_extras::{Column, TableBuilder};

    let mut navigate_to: Option<String> = None;

    // Capture selection state up-front so we can merge borders across contiguous
    // selections without conflicting mutable borrows during row rendering.
    let selection_flags: Vec<bool> = entries.iter().map(|e| e.selected).collect();

    // Clip all custom painting to the list area so nothing can overdraw the
    // properties panel on the right.
    let list_clip_rect = ui.clip_rect();

    egui::Frame::none()
        .fill(egui::Color32::TRANSPARENT)
        .inner_margin(egui::Margin {
            left: 16.0,
            right: 16.0,
            top: 0.0,
            bottom: 0.0,
        })
        .show(ui, |ui| {
            let table_id = if columns_locked {
                egui::Id::new("file_list_table_locked")
            } else {
                egui::Id::new("file_list_table_resizable")
            };

            TableBuilder::new(ui)
                .id_salt(table_id)
                // Disable striped rows to avoid tiny gaps between items.
                .striped(false)
                .resizable(!columns_locked)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(34.0)) // Checkbox
                .column(Column::remainder().at_least(220.0)) // Name
                .column(Column::exact(110.0)) // Size
                .column(Column::exact(110.0)) // Compressed
                .column(Column::exact(76.0)) // Ratio
                .column(Column::exact(140.0)) // Modified
                .column(Column::exact(36.0)) // Encrypted
                .header(28.0, |mut header| {
                    header.col(|_ui| {});
                    header.col(|ui| {
                        ui.label(
                            egui::RichText::new("Name")
                                .size(12.0)
                                .strong()
                                .color(theme.colors.text_secondary),
                        );
                    });
                    header.col(|ui| {
                        ui.label(
                            egui::RichText::new("Size")
                                .size(12.0)
                                .strong()
                                .color(theme.colors.text_secondary),
                        );
                    });
                    header.col(|ui| {
                        ui.label(
                            egui::RichText::new("Compressed")
                                .size(12.0)
                                .strong()
                                .color(theme.colors.text_secondary),
                        );
                    });
                    header.col(|ui| {
                        ui.label(
                            egui::RichText::new("Ratio")
                                .size(12.0)
                                .strong()
                                .color(theme.colors.text_secondary),
                        );
                    });
                    header.col(|ui| {
                        ui.label(
                            egui::RichText::new("Modified")
                                .size(12.0)
                                .strong()
                                .color(theme.colors.text_secondary),
                        );
                    });
                    header.col(|_ui| {});
                })
                .body(|mut body| {
                    for (row_index, entry) in entries.iter_mut().enumerate() {
                        let entry_name = entry.name.clone();
                        let is_folder = entry.is_folder;

                        body.row(32.0, |mut row| {
                            let mut checkbox_clicked = false;
                            let text_color = theme.colors.text_primary;
                            let muted_color = theme.colors.text_secondary;

                            row.col(|ui| {
                                let response = ui.checkbox(&mut entry.selected, "");
                                checkbox_clicked = response.clicked();
                            });

                            row.col(|ui| {
                                let icon = if is_folder { "📁" } else { "📄" };
                                let label = format!("{icon} {entry_name}");
                                ui.label(egui::RichText::new(label).size(14.0).color(text_color));
                            });

                            row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(&entry.size)
                                        .size(14.0)
                                        .color(text_color),
                                );
                            });

                            row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(&entry.compressed)
                                        .size(14.0)
                                        .color(text_color),
                                );
                            });

                            row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(&entry.ratio)
                                        .size(14.0)
                                        .color(text_color),
                                );
                            });

                            row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(&entry.modified)
                                        .size(14.0)
                                        .color(muted_color),
                                );
                            });

                            row.col(|ui| {
                                if entry.encrypted {
                                    ui.label(
                                        egui::RichText::new("🔒").size(14.0).color(text_color),
                                    );
                                }
                            });

                            let row_response = row.response();

                            // Paint selection highlight across the full row, clipped to the table area.
                            if entry.selected {
                                // Create a painter with the list's clip rect so row decorations cannot
                                // draw over the properties panel.
                                let painter = egui::Painter::new(
                                    row_response.ctx.clone(),
                                    row_response.layer_id,
                                    list_clip_rect,
                                );

                                // Slight horizontal inset to avoid touching table edges; expand a hair on Y
                                // to cover any default row separators that may cause tiny gaps.
                                let mut fill_rect = row_response.rect.shrink2(egui::vec2(2.0, 0.0));
                                fill_rect.min.y -= 0.5;
                                fill_rect.max.y += 0.5;

                                let fill_color = theme.colors.selection.linear_multiply(0.14);
                                painter.rect_filled(fill_rect, 0.0, fill_color);

                                // Merge borders when adjacent rows are also selected: draw only the outer
                                // top/bottom separators where neighbours are NOT selected.
                                let prev_selected = row_index > 0 && selection_flags[row_index - 1];
                                let next_selected =
                                    row_index + 1 < selection_flags.len()
                                        && selection_flags[row_index + 1];
                                let stroke_color = theme.colors.selection.linear_multiply(0.35);
                                let stroke = egui::Stroke::new(1.0, stroke_color);

                                if !prev_selected {
                                    let y = fill_rect.min.y + 0.5;
                                    painter.line_segment(
                                        [egui::pos2(fill_rect.min.x, y), egui::pos2(fill_rect.max.x, y)],
                                        stroke,
                                    );
                                }
                                if !next_selected {
                                    let y = fill_rect.max.y - 0.5;
                                    painter.line_segment(
                                        [egui::pos2(fill_rect.min.x, y), egui::pos2(fill_rect.max.x, y)],
                                        stroke,
                                    );
                                }
                            }
                            if row_response.hovered() {
                                row_response
                                    .ctx
                                    .set_cursor_icon(egui::CursorIcon::PointingHand);
                            }

                            if row_response.double_clicked() && is_folder {
                                navigate_to = Some(entry_name.clone());
                            } else if !checkbox_clicked && row_response.clicked() {
                                entry.selected = !entry.selected;
                            }
                        });
                    }
                });
        });

    navigate_to
}
