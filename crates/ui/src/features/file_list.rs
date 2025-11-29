use super::theme::AppTheme;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: String,
    pub compressed: String,
    pub ratio: String,
    pub modified: String,
    pub crc32: String,
    pub encrypted: bool,
    pub is_folder: bool,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub enum FileListAction {
    Navigate(String),
    Edit(String),   // full path (relative to current path will be resolved by caller)
    Delete(String), // same as above
    Open(String),   // double-click open file
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SortColumn {
    Name,
    Size,
    Compressed,
    Ratio,
    Modified,
    Crc32,
    Encrypted,
}

#[derive(Debug, Clone, Copy)]
pub struct SortState {
    pub column: SortColumn,
    pub ascending: bool,
}
impl Default for SortState {
    fn default() -> Self {
        Self {
            column: SortColumn::Name,
            ascending: true,
        }
    }
}

fn parse_size_to_bytes(s: &str) -> u64 {
    // Expect formats like "123 B", "12.3 KB", "4.5 MB", "1.0 GB"
    let mut parts = s.split_whitespace();
    let num_str = parts.next().unwrap_or("0");
    let unit = parts.next().unwrap_or("B").to_ascii_uppercase();
    let val: f64 = num_str.parse().unwrap_or(0.0);
    let mul = match unit.as_str() {
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (val * mul) as u64
}

fn parse_ratio_pct(s: &str) -> u64 {
    s.trim_end_matches('%').parse::<u64>().unwrap_or(0)
}

// ================= Breadcrumb =================

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
            .selectable(false)
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
            ui.add(
                egui::Label::new(
                    egui::RichText::new("/")
                        .size(14.0)
                        .color(theme.colors.text_muted),
                )
                .selectable(false),
            );

            // Split path into segments and make each clickable
            let segments: Vec<&str> = current_path.split('/').collect();
            for (idx, segment) in segments.iter().enumerate() {
                if idx > 0 {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("/")
                                .size(14.0)
                                .color(theme.colors.text_muted),
                        )
                        .selectable(false),
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
                        .selectable(false)
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

// ================= Grid View =================

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
        let ext_text: &str = if entry.is_folder { "📁" } else { &ext };

        ui.painter().text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            ext_text,
            egui::FontId::proportional(12.0),
            theme.colors.text_muted,
        );

        // File info
        let text_x = content_rect.min.x + icon_size + 12.0;
        let name_pos = egui::pos2(text_x, content_rect.min.y + 4.0);
        let meta_pos = egui::pos2(text_x, content_rect.min.y + 24.0);

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

// ================= List View (sortable + select-all) =================

fn header_sort_label(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    label: &str,
    current: SortColumn,
    this_col: SortColumn,
    ascending: bool,
) -> bool {
    let mut text = label.to_string();
    if current == this_col {
        text.push(' ');
        text.push(if ascending { '▲' } else { '▼' });
    }
    let resp = ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.0)
                .strong()
                .color(theme.colors.text_secondary),
        )
        .selectable(false)
        .sense(egui::Sense::click()),
    );
    resp.clicked()
}

fn sort_entries(entries: &mut [FileEntry], sort: &SortState) {
    let cmp = |a: &FileEntry, b: &FileEntry| -> std::cmp::Ordering {
        
        let ord = match sort.column {
            SortColumn::Name => {
                let an = a.name.to_lowercase();
                let bn = b.name.to_lowercase();
                an.cmp(&bn)
            }
            SortColumn::Size => {
                let asz = parse_size_to_bytes(&a.size);
                let bsz = parse_size_to_bytes(&b.size);
                asz.cmp(&bsz)
            }
            SortColumn::Compressed => {
                let asz = parse_size_to_bytes(&a.compressed);
                let bsz = parse_size_to_bytes(&b.compressed);
                asz.cmp(&bsz)
            }
            SortColumn::Ratio => {
                let ar = parse_ratio_pct(&a.ratio);
                let br = parse_ratio_pct(&b.ratio);
                ar.cmp(&br)
            }
            SortColumn::Modified => a.modified.cmp(&b.modified),
            SortColumn::Crc32 => {
                let an = a.crc32.to_uppercase();
                let bn = b.crc32.to_uppercase();
                an.cmp(&bn)
            }
            SortColumn::Encrypted => (a.encrypted as u8).cmp(&(b.encrypted as u8)),
        };
        if sort.ascending {
            ord
        } else {
            ord.reverse()
        }
    };
    entries.sort_by(cmp);
}

pub fn render_list_view(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    entries: &mut [FileEntry],
    columns_locked: bool,
    sort: &mut SortState,
) -> Option<FileListAction> {
    let mut action: Option<FileListAction> = None;

    // Header-driven actions to apply before drawing body
    let mut apply_select_all: Option<bool> = None;

    // Clip rectangle for row decorations (compute before building table to avoid borrow conflicts)
    let list_clip_rect = ui.clip_rect();

    egui::Frame::NONE
        .fill(egui::Color32::TRANSPARENT)
        .inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: 0,
            bottom: 0,
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
                .column(Column::exact(34.0)) // Checkbox (+ select all in header)
                .column(Column::remainder().at_least(220.0)) // Name
                .column(Column::exact(110.0)) // Size
                .column(Column::exact(110.0)) // Compressed
                .column(Column::exact(76.0)) // Ratio
                .column(Column::exact(140.0)) // Modified
                .column(Column::exact(120.0)) // CRC-32
                .column(Column::exact(36.0)) // Encrypted
                .column(Column::exact(84.0)) // Actions (Edit/Delete)
                .header(28.0, |mut header| {
                    // Select all checkbox
                    header.col(|ui| {
                        let all_selected =
                            !entries.is_empty() && entries.iter().all(|e| e.selected);
                        let some_selected = entries.iter().any(|e| e.selected);
                        let mut header_check = all_selected;
                        let resp = ui.checkbox(&mut header_check, "");
                        if resp.clicked() {
                            // Toggle to the state of header_check (clicked flips it)
                            apply_select_all = Some(header_check);
                        } else if some_selected && !all_selected && resp.hovered() {
                            // Show hint for partial selection
                            resp.on_hover_text("Some rows selected — click to toggle select all");
                        }
                    });

                    header.col(|ui| {
                        if header_sort_label(
                            ui,
                            theme,
                            "Name",
                            sort.column,
                            SortColumn::Name,
                            sort.ascending,
                        ) {
                            if sort.column == SortColumn::Name {
                                sort.ascending = !sort.ascending;
                            } else {
                                sort.column = SortColumn::Name;
                                sort.ascending = true;
                            }
                        }
                    });
                    header.col(|ui| {
                        if header_sort_label(
                            ui,
                            theme,
                            "Size",
                            sort.column,
                            SortColumn::Size,
                            sort.ascending,
                        ) {
                            if sort.column == SortColumn::Size {
                                sort.ascending = !sort.ascending;
                            } else {
                                sort.column = SortColumn::Size;
                                sort.ascending = true;
                            }
                        }
                    });
                    header.col(|ui| {
                        if header_sort_label(
                            ui,
                            theme,
                            "Compressed",
                            sort.column,
                            SortColumn::Compressed,
                            sort.ascending,
                        ) {
                            if sort.column == SortColumn::Compressed {
                                sort.ascending = !sort.ascending;
                            } else {
                                sort.column = SortColumn::Compressed;
                                sort.ascending = true;
                            }
                        }
                    });
                    header.col(|ui| {
                        if header_sort_label(
                            ui,
                            theme,
                            "Ratio",
                            sort.column,
                            SortColumn::Ratio,
                            sort.ascending,
                        ) {
                            if sort.column == SortColumn::Ratio {
                                sort.ascending = !sort.ascending;
                            } else {
                                sort.column = SortColumn::Ratio;
                                sort.ascending = true;
                            }
                        }
                    });
                    header.col(|ui| {
                        if header_sort_label(
                            ui,
                            theme,
                            "Modified",
                            sort.column,
                            SortColumn::Modified,
                            sort.ascending,
                        ) {
                            if sort.column == SortColumn::Modified {
                                sort.ascending = !sort.ascending;
                            } else {
                                sort.column = SortColumn::Modified;
                                sort.ascending = true;
                            }
                        }
                    });
                    header.col(|ui| {
                        if header_sort_label(
                            ui,
                            theme,
                            "CRC-32",
                            sort.column,
                            SortColumn::Crc32,
                            sort.ascending,
                        ) {
                            if sort.column == SortColumn::Crc32 {
                                sort.ascending = !sort.ascending;
                            } else {
                                sort.column = SortColumn::Crc32;
                                sort.ascending = true;
                            }
                        }
                    });
                    header.col(|_ui| {});
                    header.col(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("Actions")
                                    .size(12.0)
                                    .strong()
                                    .color(theme.colors.text_secondary),
                            )
                            .selectable(false),
                        );
                    });
                })
                .body(|mut body| {
                    // Apply select-all toggle if requested
                    if let Some(v) = apply_select_all.take() {
                        for e in entries.iter_mut() {
                            e.selected = v;
                        }
                    }

                    // Sort entries based on current sort state
                    sort_entries(entries, sort);

                    // Capture selection flags after sorting (for contiguous selection painting)
                    let selection_flags: Vec<bool> = entries.iter().map(|e| e.selected).collect();

                    for (row_index, entry) in entries.iter_mut().enumerate() {
                        let entry_name = entry.name.clone();
                        let is_folder = entry.is_folder;

                        body.row(32.0, |mut row| {
                            let mut checkbox_clicked = false;
                            let mut action_clicked = false;
                            let text_color = theme.colors.text_primary;
                            let muted_color = theme.colors.text_secondary;

                            row.col(|ui| {
                                let response = ui.checkbox(&mut entry.selected, "");
                                checkbox_clicked = response.clicked();
                            });

                            row.col(|ui| {
                                let icon = if is_folder { "📁" } else { "📄" };
                                let label = format!("{icon} {entry_name}");
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(label).size(14.0).color(text_color),
                                    )
                                    .selectable(false),
                                );
                            });

                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&entry.size)
                                            .size(14.0)
                                            .color(text_color),
                                    )
                                    .selectable(false),
                                );
                            });

                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&entry.compressed)
                                            .size(14.0)
                                            .color(text_color),
                                    )
                                    .selectable(false),
                                );
                            });

                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&entry.ratio)
                                            .size(14.0)
                                            .color(text_color),
                                    )
                                    .selectable(false),
                                );
                            });

                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&entry.modified)
                                            .size(14.0)
                                            .color(muted_color),
                                    )
                                    .selectable(false),
                                );
                            });

                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&entry.crc32)
                                            .size(14.0)
                                            .color(text_color),
                                    )
                                    .selectable(false),
                                );
                            });

                            row.col(|ui| {
                                if entry.encrypted {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new("🔒").size(14.0).color(text_color),
                                        )
                                        .selectable(false),
                                    );
                                }
                            });

                            // Actions column (Edit/Delete)
                            let mut pending_row_action: Option<FileListAction> = None;
                            row.col(|ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;

                                    // Edit button enabled for all files (not folders)
                                    let can_edit = !is_folder;
                                    let hover_text = if is_folder {
                                        "Cannot edit folders"
                                    } else {
                                        "Edit file (rename and/or edit content for text files)"
                                    };

                                    let edit_clicked = ui
                                        .add_enabled(
                                            can_edit,
                                            egui::Button::new("✏").min_size(egui::vec2(28.0, 22.0)),
                                        )
                                        .on_hover_text(hover_text)
                                        .clicked();
                                    if edit_clicked {
                                        action_clicked = true;
                                        pending_row_action =
                                            Some(FileListAction::Edit(entry_name.clone()));
                                    }

                                    let del_clicked = ui
                                        .add_sized(egui::vec2(28.0, 22.0), egui::Button::new("🗑"))
                                        .on_hover_text("Delete")
                                        .clicked();
                                    if del_clicked {
                                        action_clicked = true;
                                        pending_row_action =
                                            Some(FileListAction::Delete(entry_name.clone()));
                                    }
                                });
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
                                let next_selected = row_index + 1 < selection_flags.len()
                                    && selection_flags[row_index + 1];
                                let stroke_color = theme.colors.selection.linear_multiply(0.35);
                                let stroke = egui::Stroke::new(1.0, stroke_color);

                                if !prev_selected {
                                    let y = fill_rect.min.y + 0.5;
                                    painter.line_segment(
                                        [
                                            egui::pos2(fill_rect.min.x, y),
                                            egui::pos2(fill_rect.max.x, y),
                                        ],
                                        stroke,
                                    );
                                }
                                if !next_selected {
                                    let y = fill_rect.max.y - 0.5;
                                    painter.line_segment(
                                        [
                                            egui::pos2(fill_rect.min.x, y),
                                            egui::pos2(fill_rect.max.x, y),
                                        ],
                                        stroke,
                                    );
                                }
                            }
                            if row_response.hovered() {
                                row_response
                                    .ctx
                                    .set_cursor_icon(egui::CursorIcon::PointingHand);
                            }

                            if row_response.double_clicked() {
                                if is_folder {
                                    action = Some(FileListAction::Navigate(entry_name.clone()));
                                } else {
                                    action = Some(FileListAction::Open(entry_name.clone()));
                                }
                            } else if !checkbox_clicked && !action_clicked && row_response.clicked()
                            {
                                entry.selected = !entry.selected;
                            }

                            if action.is_none() {
                                if let Some(a) = pending_row_action.take() {
                                    action = Some(a);
                                }
                            }
                        });
                    }
                });
        });

    action
}
